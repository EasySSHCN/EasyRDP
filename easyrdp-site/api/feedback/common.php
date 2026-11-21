<?php
/**
 * EasyRDP 使用反馈 — 公共函数（网站端）
 *
 * 密钥与凭据存放于同目录 keys.php（PHP 退出守卫：首行 <?php exit(0); ?>，
 * 直接 HTTP 访问只会得到空响应，不会泄漏内容）。该文件属于敏感配置，
 * 不得随源码 zip / 版本库对外发布（仓库内只保留 keys.example.php 模板）。
 *
 * 反馈协议（EasySSH 同族，独立密钥）：
 *   信封 = { "v":1, "alg":"A256GCM-HS256", "ts":<ms>, "nonce":b64, "ct":b64, "tag":b64, "mac":b64 }
 *   明文 = 反馈内容 JSON（UTF-8）
 *   ct   = AES-256-GCM(encKey, nonce, 明文)
 *   mac  = HMAC-SHA256(macKey, `${ts}.${nonce}.${ct}.${tag}`)  (b64)
 *   encKey / macKey = hash_hmac('sha256', 'enc'/'mac', master) 派生
 * 服务端校验：mac → ts 时效(±15 分钟) → 解密。
 */

define('FB_SALT', 'easyrdp-feedback-v1');
// 数据目录移出 webroot：__DIR__ = /www/wwwroot/easyrdp.cn/api/feedback
// → dirname(__DIR__, 3) = /www/wwwroot → 数据落 /www/wwwroot/easyrdp-data（HTTP 不可直达）
// （注意 levels=2 会落在 webroot 内 easyrdp.cn/easyrdp-data，属安全事故，勿改回）
define('FB_DATA_DIR', dirname(__DIR__, 3) . '/easyrdp-data');
define('FB_DB', FB_DATA_DIR . '/feedback-db.json');
define('FB_KEYS', __DIR__ . '/keys.php');
define('FB_TYPES', ['功能建议', '问题反馈', '其他']);
define('FB_MAX_CONTENT', 2000);
define('FB_MAX_CONTACT', 120);
define('FB_TS_WINDOW_MS', 15 * 60 * 1000);
define('FB_RATE_PER_HOUR', 5);
define('FB_RATE_MIN_GAP_MS', 60 * 1000);

function fb_json_out($obj, $code = 200) {
  http_response_code($code);
  header('Content-Type: application/json; charset=utf-8');
  header('Cache-Control: no-store');
  echo json_encode($obj, JSON_UNESCAPED_UNICODE);
  exit;
}

function fb_fail($msg, $code = 400) { fb_json_out(['ok' => false, 'message' => $msg], $code); }

/** 读取 keys.php 中 PHP 退出守卫之后的 JSON 配置 */
function fb_keys() {
  static $keys = null;
  if ($keys !== null) return $keys;
  if (!is_readable(FB_KEYS)) fb_fail('服务端配置缺失', 500);
  $lines = file(FB_KEYS, FILE_IGNORE_NEW_LINES);
  if (!$lines || count($lines) < 2) fb_fail('服务端配置损坏', 500);
  $json = implode("\n", array_slice($lines, 1));
  $keys = json_decode($json, true);
  if (!is_array($keys) || empty($keys['master'])) fb_fail('服务端配置损坏', 500);
  return $keys;
}

function fb_b64d($s) {
  $d = base64_decode((string)$s, true);
  if ($d === false) fb_fail('信封编码非法');
  return $d;
}

/** 派生反馈加解密密钥对（HMAC-SHA256 派生，跨 PHP/Rust 行为一致，不依赖 HKDF） */
function fb_derive($masterRaw) {
  return [
    'enc' => hash_hmac('sha256', 'enc', $masterRaw, true),
    'mac' => hash_hmac('sha256', 'mac', $masterRaw, true),
  ];
}

/** 校验并解密请求信封，返回明文关联数组 */
function fb_decrypt_envelope($env) {
  if (!is_array($env) || ($env['alg'] ?? '') !== 'A256GCM-HS256' || ($env['v'] ?? 0) != 1) {
    fb_fail('信封格式非法');
  }
  $keys = fb_keys();
  $k = fb_derive(base64_decode($keys['master'], true));
  $ts = (string)($env['ts'] ?? '');
  $nonce = fb_b64d($env['nonce'] ?? '');
  $ct = fb_b64d($env['ct'] ?? '');
  $tag = fb_b64d($env['tag'] ?? '');
  $mac = fb_b64d($env['mac'] ?? '');
  if (strlen($nonce) !== 12 || strlen($tag) !== 16) fb_fail('信封参数非法');
  if (abs((int)$ts - (int)(microtime(true) * 1000)) > FB_TS_WINDOW_MS) fb_fail('请求已过期，请校对本机时间后重试');
  $want = hash_hmac('sha256', $ts . '.' . ($env['nonce'] ?? '') . '.' . ($env['ct'] ?? '') . '.' . ($env['tag'] ?? ''), $k['mac'], true);
  if (!hash_equals($want, $mac)) fb_fail('信封签名校验失败');
  $plain = openssl_decrypt($ct, 'aes-256-gcm', $k['enc'], OPENSSL_RAW_DATA, $nonce, $tag);
  if ($plain === false) fb_fail('解密失败');
  $data = json_decode($plain, true);
  if (!is_array($data)) fb_fail('解密后内容非法');
  return $data;
}

/** 读取反馈库（不存在时返回空结构） */
function fb_db_load() {
  if (!is_readable(FB_DB)) return ['items' => []];
  $j = json_decode((string)file_get_contents(FB_DB), true);
  if (!is_array($j) || !isset($j['items']) || !is_array($j['items'])) return ['items' => []];
  return $j;
}

/** 原子写回反馈库 */
function fb_db_save($db) {
  $dir = dirname(FB_DB);
  if (!is_dir($dir)) @mkdir($dir, 02775, true);
  $tmp = FB_DB . '.tmp.' . getmypid();
  if (@file_put_contents($tmp, json_encode($db, JSON_UNESCAPED_UNICODE | JSON_PRETTY_PRINT), LOCK_EX) === false) {
    fb_fail('服务器存储写入失败', 500);
  }
  @chmod($tmp, 0664);
  if (!@rename($tmp, FB_DB)) { @unlink($tmp); fb_fail('服务器存储写入失败', 500); }
}

/* ==================== 通用 JSON 小文件存取（频控/会话等） ==================== */

function fb_json_file_load($path) {
  if (!is_readable($path)) return [];
  $j = json_decode((string)file_get_contents($path), true);
  return is_array($j) ? $j : [];
}

/** 原子写任意 JSON 小文件（tmp + rename，防并发写坏） */
function fb_json_file_save($path, $data) {
  $dir = dirname($path);
  if (!is_dir($dir)) @mkdir($dir, 02775, true);
  $tmp = $path . '.tmp.' . getmypid();
  if (@file_put_contents($tmp, json_encode($data, JSON_UNESCAPED_UNICODE), LOCK_EX) === false) return false;
  @chmod($tmp, 0664);
  if (!@rename($tmp, $path)) { @unlink($tmp); return false; }
  return true;
}

/* ==================== 客户端 IP：只信任可信代理的 X-Forwarded-For ==================== */

function fb_ip_in_cidr($ip, $cidr) {
  $cidr = trim($cidr);
  if ($cidr === '') return false;
  if (strpos($cidr, '/') === false) return $ip === $cidr;
  list($sub, $bits) = explode('/', $cidr, 2);
  $bits = (int)$bits;
  if (filter_var($ip, FILTER_VALIDATE_IP, FILTER_FLAG_IPV4) && filter_var($sub, FILTER_VALIDATE_IP, FILTER_FLAG_IPV4)) {
    if ($bits < 0 || $bits > 32) return false;
    return ($bits === 0) || ((ip2long($ip) >> (32 - $bits)) === (ip2long($sub) >> (32 - $bits)));
  }
  $i6 = @inet_pton($ip); $s6 = @inet_pton($sub);
  if ($i6 === false || $s6 === false || strlen($i6) !== strlen($s6)) return false;
  if ($bits < 0 || $bits > strlen($i6) * 8) return false;
  $full = intdiv($bits, 8);
  if (substr($i6, 0, $full) !== substr($s6, 0, $full)) return false;
  $rem = $bits % 8;
  if ($rem && (ord($i6[$full]) >> (8 - $rem)) !== ($bits === strlen($i6) * 8 ? 0 : (ord($s6[$full]) >> (8 - $rem)))) return false;
  return true;
}

/** 仅当直连方是 keys.php trusted_proxies 声明的反向代理时才取 XFF 最后一跳，防伪造头绕过频控 */
function fb_client_ip() {
  $remote = $_SERVER['REMOTE_ADDR'] ?? '';
  $cfg = (string)(fb_keys()['trusted_proxies'] ?? '');
  if ($cfg === '') $cfg = (string)(getenv('FB_TRUSTED_PROXIES') ?: '');
  $trusted = array_filter(array_map('trim', explode(',', $cfg)));
  foreach ($trusted as $cidr) {
    if (fb_ip_in_cidr($remote, $cidr)) {
      $parts = array_map('trim', explode(',', $_SERVER['HTTP_X_FORWARDED_FOR'] ?? ''));
      $parts = array_values(array_filter($parts, function ($p) { return $p !== ''; }));
      if ($parts) return (string)end($parts);
      break;
    }
  }
  return $remote;
}

function fb_now_ms() { return (int)(microtime(true) * 1000); }

function fb_date_str($ms) { return date('Y-m-d H:i', (int)($ms / 1000)); }

/* ==================== 管理员会话（可吊销随机 token） ==================== */

define('FB_SESSION_TTL_MS', 12 * 3600 * 1000);
define('FB_SESSIONS_FILE', FB_DATA_DIR . '/admin-sessions.json');
define('FB_SESSIONS_MAX', 50);
define('FB_LOGIN_FAIL_FILE', FB_DATA_DIR . '/login-fail.json');
define('FB_LOGIN_FAIL_WINDOW_MS', 15 * 60 * 1000);
define('FB_LOGIN_FAIL_MAX', 5);
define('FB_CODES_FILE', FB_DATA_DIR . '/verify-codes.json');
define('FB_CAPTCHA_FILE', FB_DATA_DIR . '/captcha-store.json');
define('FB_CAPTCHA_RATE_FILE', FB_DATA_DIR . '/captcha-ratelimit.json');
define('FB_CODE_TTL_MS', 15 * 60 * 1000);
define('FB_CODE_RESEND_GAP_MS', 60 * 1000);
define('FB_CODE_HOURLY_MAX', 5);
define('FB_CODE_ATTEMPT_MAX', 5);

function fb_session_create($user) {
  $sessions = fb_json_file_load(FB_SESSIONS_FILE);
  $now = fb_now_ms();
  foreach ($sessions as $t => $s) {
    if (!is_array($s) || (int)($s['expires'] ?? 0) <= $now) unset($sessions[$t]);
  }
  if (count($sessions) >= FB_SESSIONS_MAX) {
    uasort($sessions, function ($a, $b) { return (int)($a['expires'] ?? 0) <=> (int)($b['expires'] ?? 0); });
    $sessions = array_slice($sessions, count($sessions) - FB_SESSIONS_MAX + 1, null, true);
  }
  $token = bin2hex(random_bytes(32));
  $sessions[$token] = ['user' => (string)$user, 'expires' => $now + FB_SESSION_TTL_MS, 'created' => $now];
  fb_json_file_save(FB_SESSIONS_FILE, $sessions);
  return $token;
}

function fb_check_token($user, $token) {
  if (!is_string($token) || strlen($token) !== 64 || !ctype_xdigit($token)) return false;
  $sessions = fb_json_file_load(FB_SESSIONS_FILE);
  $s = $sessions[$token] ?? null;
  if (!is_array($s)) return false;
  if ((string)($s['user'] ?? '') !== (string)$user) return false;
  if ((int)($s['expires'] ?? 0) <= fb_now_ms()) return false;
  return true;
}

function fb_sessions_revoke_all() {
  fb_json_file_save(FB_SESSIONS_FILE, []);
}

/* ==================== 登录失败限速（按 IP+用户名） ==================== */

function fb_login_fail_key($ip, $user) {
  return hash('sha256', $ip . '|' . $user . '|' . (string)(fb_keys()['token_secret'] ?? ''));
}

function fb_login_fail_hit($ip, $user) {
  $now = fb_now_ms();
  $db = fb_json_file_load(FB_LOGIN_FAIL_FILE);
  $rec = $db[fb_login_fail_key($ip, $user)] ?? null;
  if (is_array($rec) && (int)($rec['window'] ?? 0) > $now - FB_LOGIN_FAIL_WINDOW_MS) {
    return (int)($rec['count'] ?? 0) >= FB_LOGIN_FAIL_MAX;
  }
  return false;
}

function fb_login_fail_record($ip, $user) {
  $now = fb_now_ms();
  $db = fb_json_file_load(FB_LOGIN_FAIL_FILE);
  $key = fb_login_fail_key($ip, $user);
  $rec = $db[$key] ?? null;
  if (is_array($rec) && (int)($rec['window'] ?? 0) > $now - FB_LOGIN_FAIL_WINDOW_MS) {
    $rec['count'] = (int)($rec['count'] ?? 0) + 1;
    $rec['window'] = max((int)$rec['window'], $now);
  } else {
    $rec = ['count' => 1, 'window' => $now];
  }
  $db[$key] = $rec;
  foreach ($db as $k => $r) {
    if (!is_array($r) || (int)($r['window'] ?? 0) <= $now - FB_LOGIN_FAIL_WINDOW_MS) unset($db[$k]);
  }
  if (count($db) > 5000) $db = array_slice($db, -5000, null, true);
  fb_json_file_save($db ? FB_LOGIN_FAIL_FILE : FB_LOGIN_FAIL_FILE, $db);
}

function fb_login_fail_reset($ip, $user) {
  $db = fb_json_file_load(FB_LOGIN_FAIL_FILE);
  unset($db[fb_login_fail_key($ip, $user)]);
  fb_json_file_save(FB_LOGIN_FAIL_FILE, $db);
}

/* ==================== 邮件发送（管理后台验证码通道） ==================== */

function fb_mail_send($to, $subject, $text) {
  $mail = fb_keys()['mail'] ?? null;
  if (!is_array($mail) || empty($mail['url']) || empty($mail['key'])) return '邮件通道未配置';
  $url = (string)$mail['url'];
  if (substr($url, -8) !== '/v1/send') $url = rtrim($url, '/') . '/v1/send';
  $body = json_encode(['to' => [(string)$to], 'subject' => (string)$subject, 'text' => (string)$text], JSON_UNESCAPED_UNICODE);
  $ch = curl_init($url);
  curl_setopt_array($ch, [
    CURLOPT_POST => true,
    CURLOPT_POSTFIELDS => $body,
    CURLOPT_RETURNTRANSFER => true,
    CURLOPT_TIMEOUT => 20,
    CURLOPT_HTTPHEADER => ['Content-Type: application/json; charset=utf-8', 'Authorization: Bearer ' . (string)$mail['key']],
  ]);
  $res = curl_exec($ch);
  $err = curl_errno($ch) ? curl_error($ch) : '';
  $code = (int)curl_getinfo($ch, CURLINFO_RESPONSE_CODE);
  curl_close($ch);
  if ($res === false) return '邮件发送失败:' . $err;
  $j = json_decode((string)$res, true);
  if ($code >= 400 || !is_array($j) || empty($j['ok'])) return '邮件发送失败:' . (string)($j['message'] ?? ('HTTP ' . $code));
  return true;
}

/* ==================== 邮箱验证码（绑定/找回共用，一次性消费） ==================== */

function fb_code_hash($code) {
  return hash('sha256', (string)$code . '|' . (string)(fb_keys()['token_secret'] ?? ''));
}

function fb_code_issue($key, $ttlMs = FB_CODE_TTL_MS) {
  $now = fb_now_ms();
  $db = fb_json_file_load(FB_CODES_FILE);
  foreach ($db as $k => $v) if (!is_array($v) || (int)($v['expires'] ?? 0) <= $now) unset($db[$k]);
  $sent = [];
  $rec = $db[$key] ?? null;
  if (is_array($rec)) {
    if ($now - (int)($rec['issuedAt'] ?? 0) < FB_CODE_RESEND_GAP_MS) {
      return ['ok' => false, 'message' => '发送过于频繁,请 1 分钟后再试'];
    }
    $sent = array_values(array_filter(is_array($rec['sent'] ?? null) ? $rec['sent'] : [],
      function ($t) use ($now) { return $t > $now - 3600 * 1000; }));
    if (count($sent) >= FB_CODE_HOURLY_MAX) {
      return ['ok' => false, 'message' => '该目标发送次数已达上限,请稍后再试'];
    }
  }
  if (count($db) > 500) $db = array_slice($db, -500, null, true);
  $code = str_pad((string)random_int(0, 999999), 6, '0', STR_PAD_LEFT);
  $db[$key] = [
    'hash' => fb_code_hash($code),
    'expires' => $now + $ttlMs,
    'attempts' => 0,
    'issuedAt' => $now,
    'sent' => array_merge($sent, [$now]),
  ];
  fb_json_file_save(FB_CODES_FILE, $db);
  return ['ok' => true, 'code' => $code];
}

function fb_code_verify($key, $code, $maxAttempts = FB_CODE_ATTEMPT_MAX) {
  $now = fb_now_ms();
  $db = fb_json_file_load(FB_CODES_FILE);
  $rec = $db[$key] ?? null;
  if (!is_array($rec)) return '请先获取验证码';
  if ((int)($rec['expires'] ?? 0) <= $now) { unset($db[$key]); fb_json_file_save(FB_CODES_FILE, $db); return '验证码已过期,请重新获取'; }
  if ((int)($rec['attempts'] ?? 0) >= $maxAttempts) { unset($db[$key]); fb_json_file_save(FB_CODES_FILE, $db); return '验证码错误次数过多,请重新获取'; }
  $rec['attempts'] = (int)($rec['attempts'] ?? 0) + 1;
  $ok = is_string($code) && strlen($code) === 6 && ctype_digit($code)
    && hash_equals((string)($rec['hash'] ?? ''), fb_code_hash($code));
  if (!$ok) { $db[$key] = $rec; fb_json_file_save(FB_CODES_FILE, $db); return '验证码错误'; }
  unset($db[$key]);
  fb_json_file_save(FB_CODES_FILE, $db);
  return true;
}

/* ==================== 图形验证码 ==================== */

/** 按 IP 滑动窗口频控。返回 true=放行,false=超限 */
function fb_ip_rate_ok($file, $max, $windowMs = 3600000, $maxKeys = 5000) {
  $key = hash('sha256', (string)fb_client_ip());
  $now = fb_now_ms();
  $db = fb_json_file_load($file);
  $list = array_values(array_filter(is_array($db[$key] ?? null) ? $db[$key] : [],
    function ($t) use ($now, $windowMs) { return $t > $now - $windowMs; }));
  if (count($list) >= $max) return false;
  $list[] = $now;
  $db[$key] = $list;
  foreach ($db as $k => $arr) {
    $db[$k] = array_values(array_filter(is_array($arr) ? $arr : [],
      function ($t) use ($now, $windowMs) { return $t > $now - $windowMs; }));
    if (!count($db[$k])) unset($db[$k]);
  }
  if (count($db) > $maxKeys) $db = array_slice($db, -$maxKeys, null, true);
  fb_json_file_save($file, $db);
  return true;
}

/** 校验并消费图形验证码。返回 true 或错误消息字符串(不区分大小写) */
function fb_captcha_verify($id, $code) {
  $now = fb_now_ms();
  $db = fb_json_file_load(FB_CAPTCHA_FILE);
  $rec = $db[(string)$id] ?? null;
  if (!is_array($rec)) return '图形验证码无效,请刷新后重试';
  unset($db[(string)$id]); // 一次性:无论对错都销毁,防重放
  fb_json_file_save(FB_CAPTCHA_FILE, $db);
  if ((int)($rec['expires'] ?? 0) <= $now) return '图形验证码已过期,请刷新后重试';
  if (!is_string($code) || strlen($code) < 4 || strlen($code) > 8
    || !hash_equals((string)($rec['code'] ?? ''), strtolower(trim($code)))) {
    return '图形验证码错误';
  }
  return true;
}

/* ==================== 管理员绑定邮箱独立存储（部署不覆盖） ==================== */

define('FB_ADMIN_META', FB_DATA_DIR . '/admin-meta.json');

function fb_admin_meta_load() {
  $j = fb_json_file_load(FB_ADMIN_META);
  return is_array($j) ? $j : [];
}

function fb_admin_email() {
  $meta = fb_admin_meta_load();
  $e = trim((string)($meta['admin_email'] ?? ''));
  if ($e !== '') return $e;
  return trim((string)(fb_keys()['admin_email'] ?? ''));
}

function fb_admin_email_set($email) {
  $meta = fb_admin_meta_load();
  $meta['admin_email'] = (string)$email;
  $meta['updatedAt'] = fb_now_ms();
  return fb_json_file_save(FB_ADMIN_META, $meta) ? true : '绑定信息写入失败';
}

/** 原子保存 keys.php（保持 PHP 退出守卫行） */
function fb_keys_save($keys) {
  $cur = fb_keys();
  $new = [
    'master'      => (string)$cur['master'],
    'admin_user'  => (string)($keys['admin_user'] ?? $cur['admin_user']),
    'admin_hash'  => (string)($keys['admin_hash'] ?? $cur['admin_hash']),
    'token_secret'=> (string)$cur['token_secret'],
    'trusted_proxies' => (string)($cur['trusted_proxies'] ?? ''),
    'admin_email' => (string)($keys['admin_email'] ?? ($cur['admin_email'] ?? '')),
    'mail'        => is_array($cur['mail'] ?? null) ? $cur['mail'] : [],
  ];
  if (strlen($new['admin_user']) < 4 || mb_strlen($new['admin_user']) > 40) return '新用户名需 4-40 个字符';
  if (strpos($new['admin_hash'], '$2y$') !== 0 && strpos($new['admin_hash'], '$2a$') !== 0) return '哈希格式非法';
  $out = "<?php exit(0); ?>\n" . json_encode($new, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES) . "\n";
  $tmp = FB_KEYS . '.tmp.' . getmypid();
  if (@file_put_contents($tmp, $out, LOCK_EX) === false) return '凭据文件写入失败';
  @chmod($tmp, 0660);
  if (!@rename($tmp, FB_KEYS)) { @unlink($tmp); return '凭据文件替换失败'; }
  return true;
}
