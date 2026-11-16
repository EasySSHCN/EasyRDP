<?php
/**
 * /api/update/check.php —— 在线更新检测接口（需求 R-20261116-005）
 *
 * 协议（与客户端 updater 约定，见 docs\方案-在线更新机制.md）：
 *   请求体 = JSON 信封 { "v":1, "alg":"A256GCM-HS256", "ts":<ms>, "nonce":b64, "ct":b64, "tag":b64, "mac":b64 }
 *   ct     = AES-256-GCM(encKey, nonce, 明文)
 *   mac    = HMAC-SHA256(macKey, `${ts}.${nonce}.${ct}.${tag}`)  (b64)
 *   ★ update 派生 salt 规则（与 feedback 族不同，防跨接口密钥复用）：
 *       encKey = hash_hmac('sha256', 'update-enc', master, true)
 *       macKey = hash_hmac('sha256', 'update-mac', master, true)
 *   master 存同目录 keys.php（PHP 退出守卫，结构同 feedback/keys.example.php）。
 *
 * 服务端流程：mac 校验 → ts 时效(±15 分钟) → AES-256-GCM 解密（请求明文仅作心跳/版本上下文，
 * 不强制要求内容）→ 读同目录 latest.json（发布流程第 9 步 ota-package.ps1 生成，字段
 * latestVersion/file/notes/sha256/sig）→ 以同族信封加密返回。
 * latest.json 缺失或损坏时返回 {latestVersion:""} 信封（客户端视为「暂无更新」）。
 */

header('Content-Type: application/json; charset=utf-8');
header('Cache-Control: no-store');

define('UP_TS_WINDOW_MS', 15 * 60 * 1000);
define('UP_LATEST', __DIR__ . '/latest.json');
define('UP_KEYS', __DIR__ . '/keys.php');

/** 明文 JSON 错误输出（错误不进信封，与 feedback 族一致） */
function up_fail($msg, $code = 400) {
  http_response_code($code);
  echo json_encode(['ok' => false, 'message' => $msg], JSON_UNESCAPED_UNICODE);
  exit;
}

function up_b64d($s) {
  $d = base64_decode((string)$s, true);
  if ($d === false) up_fail('信封编码非法');
  return $d;
}

/** 读取 keys.php 中退出守卫行之后的 JSON 配置 */
function up_keys() {
  static $keys = null;
  if ($keys !== null) return $keys;
  if (!is_readable(UP_KEYS)) up_fail('服务端配置缺失', 500);
  $lines = file(UP_KEYS, FILE_IGNORE_NEW_LINES);
  if (!$lines || count($lines) < 2) up_fail('服务端配置损坏', 500);
  $json = implode("\n", array_slice($lines, 1));
  $keys = json_decode($json, true);
  if (!is_array($keys) || empty($keys['master'])) up_fail('服务端配置损坏', 500);
  return $keys;
}

/** ★ update 族派生密钥（salt: update-enc / update-mac；与客户端约定一致，勿改） */
function up_derive($masterRaw) {
  return [
    'enc' => hash_hmac('sha256', 'update-enc', $masterRaw, true),
    'mac' => hash_hmac('sha256', 'update-mac', $masterRaw, true),
  ];
}

/** 校验并解密请求信封，返回明文（内容不强制为 JSON） */
function up_decrypt_envelope() {
  $raw = (string)file_get_contents('php://input');
  if ($raw === '') up_fail('请求体为空');
  $env = json_decode($raw, true);
  if (!is_array($env) || ($env['v'] ?? 0) != 1 || ($env['alg'] ?? '') !== 'A256GCM-HS256') {
    up_fail('信封格式非法');
  }
  $k = up_derive(base64_decode(up_keys()['master'], true));
  $ts    = (string)($env['ts'] ?? '');
  $nonce = up_b64d($env['nonce'] ?? '');
  $ct    = up_b64d($env['ct'] ?? '');
  $tag   = up_b64d($env['tag'] ?? '');
  $mac   = up_b64d($env['mac'] ?? '');
  if (strlen($nonce) !== 12 || strlen($tag) !== 16) up_fail('信封参数非法');
  if ($ts === '' || !ctype_digit($ts) || abs((int)$ts - (int)(microtime(true) * 1000)) > UP_TS_WINDOW_MS) {
    up_fail('请求已过期，请校对本机时间后重试');
  }
  $want = hash_hmac('sha256', $ts . '.' . ($env['nonce'] ?? '') . '.' . ($env['ct'] ?? '') . '.' . ($env['tag'] ?? ''), $k['mac'], true);
  if (!hash_equals($want, $mac)) up_fail('信封签名校验失败');
  $plain = openssl_decrypt($ct, 'aes-256-gcm', $k['enc'], OPENSSL_RAW_DATA, $nonce, $tag);
  if ($plain === false) up_fail('解密失败');
  return $plain;
}

/** 用 update 族信封加密任意明文并直接输出（ts=当前 ms） */
function up_emit_envelope($plain) {
  $k = up_derive(base64_decode(up_keys()['master'], true));
  $ts = (string)(int)(microtime(true) * 1000);
  $nonce = random_bytes(12);
  $tag = '';
  $ct = openssl_encrypt($plain, 'aes-256-gcm', $k['enc'], OPENSSL_RAW_DATA, $nonce, $tag);
  if ($ct === false) up_fail('服务端加密失败', 500);
  $nonceB = base64_encode($nonce);
  $ctB = base64_encode($ct);
  $tagB = base64_encode($tag);
  $mac = base64_encode(hash_hmac('sha256', $ts . '.' . $nonceB . '.' . $ctB . '.' . $tagB, $k['mac'], true));
  echo json_encode([
    'v' => 1, 'alg' => 'A256GCM-HS256', 'ts' => (int)$ts,
    'nonce' => $nonceB, 'ct' => $ctB, 'tag' => $tagB, 'mac' => $mac,
  ], JSON_UNESCAPED_SLASHES);
  exit;
}

/* ==================== 主流程 ==================== */

up_decrypt_envelope(); // 校验通过才继续；失败已在函数内 exit

// 读发布流程生成的 latest.json（不存在/损坏 → 「暂无更新」空响应，不报错）
$payload = ['latestVersion' => ''];
if (is_readable(UP_LATEST)) {
  $j = json_decode((string)file_get_contents(UP_LATEST), true);
  if (is_array($j) && isset($j['latestVersion']) && $j['latestVersion'] !== '') {
    $payload = [
      'latestVersion' => (string)$j['latestVersion'],
      'file'          => (string)($j['file'] ?? ''),
      'notes'         => (string)($j['notes'] ?? ''),
      'sha256'        => (string)($j['sha256'] ?? ''),
      'sig'           => (string)($j['sig'] ?? ''),
    ];
  }
}

up_emit_envelope(json_encode($payload, JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE));
