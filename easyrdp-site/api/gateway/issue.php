<?php
/**
 * POST /api/gateway/issue.php —— 用户侧云网关自助服务台（EasyRDP · R-20261120-009）
 *
 * 动作（POST JSON，cookie 会话鉴权）：
 *   { action:"register",  email, password }        → { ok:true, message:"..." }（密码 ≥10 位）
 *   { action:"login",     email, password }        → { ok:true, email }
 *   { action:"my_resources" }                      → { ok:true, resources:[{resource_id,label}] }（未登录 401）
 *   { action:"issue",     resource_id }            → { ok, token, resource_id, expires_in, gateway:{host,port}, hint }
 *                                                    （须有 grants 授权；服务端内网回环调网关 POST /admin/issue，
 *                                                      X-Admin-Key，sub=邮箱，roles=["user"]，ttl_seconds=3600 上限）
 *   { action:"revoke",    resource_id }            → { ok:true, message }
 *
 * 配置：__DIR__/keys.php（参照 keys.example.php 模板人工放置；真实密钥绝不入库）：
 *   gateway_url         网关回环地址（如 http://127.0.0.1:8443）
 *   gateway_admin_key   与 gateway config.toml [admin].admin_key 一致的共享密钥
 *   users_db            可选，SQLite 路径（默认 /var/lib/easyrdp-web/users.db）
 *   gateway_host/port   可选，返回给客户端的网关地址（默认 easyrdp.cn / 443）
 *
 * 数据库自举：users / grants 两表首次访问自动 CREATE TABLE IF NOT EXISTS
 * （对齐 RELEASE.md GW-FIX-V042 部署清单④，无需单独初始化脚本）。
 */

header('Content-Type: application/json; charset=utf-8');
header('Cache-Control: no-store');

function jfail(string $msg, int $status = 400): void {
  http_response_code($status);
  echo json_encode(['ok' => false, 'message' => $msg], JSON_UNESCAPED_UNICODE);
  exit;
}

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') jfail('仅支持 POST', 405);
$in = json_decode((string)file_get_contents('php://input'), true);
if (!is_array($in)) jfail('请求体必须是 JSON');
$action = (string)($in['action'] ?? '');

/* ---------- 配置 ---------- */
$keysFile = __DIR__ . '/keys.php';
if (!is_readable($keysFile)) jfail('网关配置缺失（api/gateway/keys.php 未放置），请联系管理员', 500);
$keys = json_decode((string)preg_replace('/^<\?php exit\(0\); \?>\s*/', '', (string)file_get_contents($keysFile)), true);
if (!is_array($keys) || empty($keys['gateway_url']) || empty($keys['gateway_admin_key'])) {
  jfail('网关配置不完整（gateway_url / gateway_admin_key 缺失）', 500);
}
$gwBase = rtrim((string)$keys['gateway_url'], '/');
$gwKey  = (string)$keys['gateway_admin_key'];
$gwHost = (string)($keys['gateway_host'] ?? 'easyrdp.cn');
$gwPort = (int)($keys['gateway_port'] ?? 443);
$usersDb = (string)($keys['users_db'] ?? '/var/lib/easyrdp-web/users.db');

/* ---------- 会话（PHP session，cookie 鉴权，服务端超时回收） ---------- */
session_name('GWSID');
session_start();
$gwEmail = $_SESSION['gw_email'] ?? '';
function require_login(): string {
  global $gwEmail;
  if ($gwEmail === '' ) jfail('未登录或登录已过期，请重新登录', 401);
  return $gwEmail;
}

/* ---------- SQLite（users / grants，自举建表） ---------- */
try {
  $dbDir = dirname($usersDb);
  if (!is_dir($dbDir)) @mkdir($dbDir, 0750, true);
  $db = new PDO('sqlite:' . $usersDb, null, null, [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]);
  $db->exec('CREATE TABLE IF NOT EXISTS users (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      email TEXT NOT NULL UNIQUE,
      pass_hash TEXT NOT NULL,
      created_at TEXT NOT NULL)');
  $db->exec('CREATE TABLE IF NOT EXISTS grants (
      user_id INTEGER NOT NULL,
      resource_id TEXT NOT NULL,
      label TEXT NOT NULL DEFAULT "",
      granted_at TEXT NOT NULL,
      PRIMARY KEY (user_id, resource_id))');
} catch (Throwable $e) {
  jfail('用户库不可用，请联系管理员', 500);
}

$validEmail = fn(string $e): bool => (bool)preg_match('/^[^@\s]{1,64}@[^@\s]{1,255}\.[A-Za-z]{2,24}$/', $e);

/* ---------- 动作分发 ---------- */
switch ($action) {
  case 'register': {
    $email = trim((string)($in['email'] ?? ''));
    $pass  = (string)($in['password'] ?? '');
    if (!$validEmail($email)) jfail('邮箱格式不正确');
    if (strlen($pass) < 10) jfail('密码至少 10 位');
    $st = $db->prepare('SELECT id FROM users WHERE email = ?');
    $st->execute([$email]);
    if ($st->fetch()) jfail('该邮箱已注册，请直接登录');
    $db->prepare('INSERT INTO users(email, pass_hash, created_at) VALUES(?,?,?)')
       ->execute([$email, password_hash($pass, PASSWORD_DEFAULT), gmdate('Y-m-d\TH:i:s\Z')]);
    echo json_encode(['ok' => true, 'message' => '注册成功，请登录'], JSON_UNESCAPED_UNICODE);
    exit;
  }

  case 'login': {
    $email = trim((string)($in['email'] ?? ''));
    $pass  = (string)($in['password'] ?? '');
    if (!$validEmail($email) || $pass === '') jfail('请填写邮箱和密码');
    $st = $db->prepare('SELECT id, pass_hash FROM users WHERE email = ?');
    $st->execute([$email]);
    $row = $st->fetch(PDO::FETCH_ASSOC);
    if (!$row || !password_verify($pass, (string)$row['pass_hash'])) jfail('邮箱或密码不正确', 401);
    session_regenerate_id(true);
    $_SESSION['gw_email'] = $email;
    $_SESSION['gw_uid'] = (int)$row['id'];
    echo json_encode(['ok' => true, 'email' => $email]);
    exit;
  }

  case 'my_resources': {
    require_login();
    $st = $db->prepare('SELECT resource_id, label FROM grants WHERE user_id = ? ORDER BY granted_at');
    $st->execute([(int)$_SESSION['gw_uid']]);
    $res = [];
    foreach ($st->fetchAll(PDO::FETCH_ASSOC) as $r) {
      $res[] = ['resource_id' => (string)$r['resource_id'], 'label' => (string)$r['label']];
    }
    echo json_encode(['ok' => true, 'resources' => $res], JSON_UNESCAPED_UNICODE);
    exit;
  }

  case 'issue': {
    require_login();
    $rid = trim((string)($in['resource_id'] ?? ''));
    if ($rid === '' || strlen($rid) > 128) jfail('resource_id 缺失或超长');
    $st = $db->prepare('SELECT label FROM grants WHERE user_id = ? AND resource_id = ?');
    $st->execute([(int)$_SESSION['gw_uid'], $rid]);
    $label = $st->fetchColumn();
    if ($label === false) jfail('该资源未对你开放授权，请联系官方开通', 403);
    /* 内网回环调网关 POST /admin/issue（X-Admin-Key；TTL 3600s，网关侧再钳制 ≤3600） */
    $body = json_encode(['sub' => $_SESSION['gw_email'], 'roles' => ['user'], 'ttl_seconds' => 3600]);
    $ctx = stream_context_create(['http' => [
      'method'  => 'POST',
      'timeout' => 5,
      'header'  => "Content-Type: application/json\r\nX-Admin-Key: " . $gwKey . "\r\nUser-Agent: easyrdp-site-issue\r\n",
      'content' => $body,
      'ignore_errors' => true,
    ]]);
    $resp = @file_get_contents($gwBase . '/admin/issue', false, $ctx);
    if ($resp === false) jfail('网关不可达，请稍后重试', 502);
    $j = json_decode((string)$resp, true);
    if (!is_array($j) || ($j['ok'] ?? false) !== true || empty($j['token'])) {
      $msg = is_array($j) && isset($j['message']) ? (string)$j['message'] : '网关签发失败';
      jfail('网关返回错误：' . $msg, 502);
    }
    echo json_encode([
      'ok' => true,
      'token' => (string)$j['token'],
      'resource_id' => $rid,
      'expires_in' => (int)($j['expires_in'] ?? 3600),
      'gateway' => ['host' => $gwHost, 'port' => $gwPort],
      'hint' => '在客户端「网关（可选）」中按以上参数填写，认证方式选 Token（JWT）',
    ], JSON_UNESCAPED_UNICODE);
    exit;
  }

  case 'revoke': {
    require_login();
    $rid = trim((string)($in['resource_id'] ?? ''));
    if ($rid === '' || strlen($rid) > 128) jfail('resource_id 缺失或超长');
    $db->prepare('DELETE FROM grants WHERE user_id = ? AND resource_id = ?')
       ->execute([(int)$_SESSION['gw_uid'], $rid]);
    echo json_encode(['ok' => true, 'message' => '授权已吊销（如需恢复请联系官方重新开通）'], JSON_UNESCAPED_UNICODE);
    exit;
  }

  default:
    jfail('未知 action');
}
