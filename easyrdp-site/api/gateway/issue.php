<?php
/**
 * POST /api/gateway/issue.php —— 用户侧云网关自助服务台（EasyRDP · R-20261120-009）
 *
 * 动作（POST JSON，cookie 会话鉴权）：
 *   { action:"register",  email, password }        → { ok:true, message:"..." }（密码 ≥10 位）
 *   { action:"login",     email, password }        → { ok:true, email }
 *   { action:"my_resources" }                      → { ok:true, resources:[{resource_id,label}] }（未登录 401）
 *   { action:"issue",     resource_id, ttl_seconds? } → { ok, token, resource_id, expires_in, gateway:{host,port}, hint }
 *                                                    （须有 grants 授权；服务端内网回环调网关 POST /admin/issue，
 *                                                      X-Admin-Key，sub=邮箱，roles=["user"]，ttl_seconds 白名单自定义，默认 3600）
 *   { action:"revoke",    resource_id }            → { ok:true, message }
 *
 * 配置：__DIR__/keys.php（参照 keys.example.php 模板人工放置；真实密钥绝不入库）：
 *   gateway_url         网关回环地址（如 http://127.0.0.1:8443）
 *   gateway_admin_key   与 gateway config.toml [admin].admin_key 一致的共享密钥
 *   users_db            可选，SQLite 路径（默认 /www/wwwroot/easyrdp-data/users.db，
 *                       该目录在 open_basedir 白名单内且不在 webroot 下，不可被 Web 直读）
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
/* GW-MAILVERIFY-20261121：资源注册表（与 grants-admin/网关 config 对齐，验证通过后自动审批第一项） */
$resources = $keys['resources'] ?? [['id' => 'rdp-win01', 'label' => 'Windows 桌板']];
$usersDb = (string)($keys['users_db'] ?? '/www/wwwroot/easyrdp-data/users.db');

/* ---------- 会话（PHP session，cookie 鉴权，服务端超时回收） ---------- */
session_name('GWSID');
session_start();
$gwEmail = $_SESSION['gw_email'] ?? '';
function require_login(): string {
  global $gwEmail, $db;
  if ($gwEmail === '' ) jfail('未登录或登录已过期，请重新登录', 401);
  /* GW-SESSIONCHK-20261122：会话必须对应现存的 active 用户——数据被清空/
     用户被级联删除/账号注销后，旧 GWSID 会话立即失效并强制回登录口，
     避免"幽灵会话"刷新后仍显示已登录（verify_status 恢复链路同样被拦） */
  $st = $db->prepare('SELECT status FROM users WHERE id = ?');
  $st->execute([(int)($_SESSION['gw_uid'] ?? 0)]);
  $row = $st->fetch(PDO::FETCH_ASSOC);
  if ($row === false || (($row['status'] ?? 'active') !== 'active')) {
    $_SESSION = [];
    session_destroy();
    jfail('登录状态已失效（账号不存在或已注销），请重新登录', 401);
  }
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
  /* GW-LOG-20261121：用户操作日志（注册/登录/退出/签发/吊销等全链路留痕，
     供官网后台「日志」弹窗分页展示；上限 50000 条，超出滚动清理最旧记录） */
  $db->exec('CREATE TABLE IF NOT EXISTS user_ops (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id INTEGER,
      email TEXT NOT NULL,
      op TEXT NOT NULL,
      detail TEXT NOT NULL DEFAULT "",
      ip TEXT NOT NULL DEFAULT "",
      ts TEXT NOT NULL)');
  /* GW-SELFDEL-20261121：账号状态列（active=正常 / self_deleted=用户自杀注销） */
  try {
    $db->exec("ALTER TABLE users ADD COLUMN status TEXT NOT NULL DEFAULT 'active'");
  } catch (Throwable $e) { /* 列已存在 */ }
  /* GW-MAILVERIFY-20261121：邮箱有效性验证列 */
  try {
    $db->exec("ALTER TABLE users ADD COLUMN email_verified INTEGER NOT NULL DEFAULT 0");
    $db->exec("ALTER TABLE users ADD COLUMN verify_code TEXT NOT NULL DEFAULT ''");
    $db->exec("ALTER TABLE users ADD COLUMN verify_expire INTEGER NOT NULL DEFAULT 0");
    $db->exec("ALTER TABLE users ADD COLUMN verify_last INTEGER NOT NULL DEFAULT 0");
  } catch (Throwable $e) { /* 列已存在 */ }
} catch (Throwable $e) {
  jfail('用户库不可用，请联系管理员', 500);
}

/* ---------- 邮箱验证码（GW-MAILVERIFY-20261121） ---------- */
/* 发送验证码：60s 重发间隔，10 分钟有效；复用反馈通道 fb_mail_send */
function gw_send_verify_mail(PDO $db, int $uid, string $email): array {
  $st = $db->prepare('SELECT email_verified, verify_last FROM users WHERE id = ?');
  $st->execute([$uid]);
  $row = $st->fetch(PDO::FETCH_ASSOC);
  if (!$row) return ['ok' => false, 'message' => '用户不存在'];
  if ((int)$row['email_verified'] === 1) return ['ok' => false, 'message' => '邮箱已完成验证'];
  $now = time();
  if ($now - (int)$row['verify_last'] < 60) {
    return ['ok' => false, 'message' => '发送过于频繁，请 ' . (60 - ($now - (int)$row['verify_last'])) . ' 秒后再试'];
  }
  $code = (string)random_int(100000, 999999);
  $db->prepare('UPDATE users SET verify_code = ?, verify_expire = ?, verify_last = ? WHERE id = ?')
     ->execute([hash('sha256', $code), $now + 600, $now, $uid]);
  $sent = fb_mail_send($email, 'EasyRDP 云网关 — 邮箱验证码',
    "您好：\n\n您的云网关邮箱验证码为：{$code}\n（10 分钟内有效；如非本人操作请忽略本邮件）\n\n—— EasyRDP 云网关\n" . gmdate('Y-m-d\TH:i:s\Z'));
  gw_op_log($db, $email, 'verify_send', $sent === true ? '邮箱验证码已发送' : '邮箱验证码发送失败：' . (string)$sent);
  return ['ok' => $sent === true, 'message' => $sent === true ? '验证码已发送，请查收邮箱' : ('邮件发送失败：' . (string)$sent)];
}

/* 校验验证码 → 通过后：标注已验证 + 自动审批默认资源 + 全链路留痕 */
function gw_verify_code(PDO $db, int $uid, string $email, string $code): array {
  global $resources;
  $st = $db->prepare('SELECT email_verified, verify_code, verify_expire FROM users WHERE id = ?');
  $st->execute([$uid]);
  $row = $st->fetch(PDO::FETCH_ASSOC);
  if (!$row) return ['ok' => false, 'message' => '用户不存在'];
  if ((int)$row['email_verified'] === 1) return ['ok' => false, 'message' => '邮箱已完成验证'];
  if ((int)$row['verify_expire'] < time()) return ['ok' => false, 'message' => '验证码已过期，请重新发送'];
  if (hash('sha256', trim($code)) !== (string)$row['verify_code']) return ['ok' => false, 'message' => '验证码不正确'];
  $db->prepare("UPDATE users SET email_verified = 1, verify_code = '', verify_expire = 0 WHERE id = ?")
     ->execute([$uid]);
  gw_op_log($db, $email, 'email_verified', '邮箱验证通过');
  gw_audit_record($email, 'email_verified');
  /* 邮箱验证通过 → 联动自动审批默认资源（资源列表第一项） */
  $res0 = $resources[0] ?? null;
  $granted = '';
  if (is_array($res0) && !empty($res0['id']) && preg_match('/^[A-Za-z0-9._-]{1,64}$/', (string)$res0['id'])) {
    $rid = (string)$res0['id'];
    $label = (string)($res0['label'] ?? '');
    $ins = $db->prepare('INSERT OR IGNORE INTO grants(user_id, resource_id, label, granted_at) VALUES(?,?,?,?)');
    $ins->execute([$uid, $rid, $label, gmdate('Y-m-d\TH:i:s\Z')]);
    if ($ins->rowCount() === 0) {
      $db->prepare('UPDATE grants SET label = ? WHERE user_id = ? AND resource_id = ?')
         ->execute([$label, $uid, $rid]);
    }
    $granted = $rid;
    gw_op_log($db, $email, 'auto_grant', "邮箱验证联动自动审批资源：{$rid}");
    gw_audit_record($email, 'resource_auto_granted', $rid);
  }
  return ['ok' => true, 'granted' => $granted,
    'message' => '邮箱验证通过' . ($granted ? "，已自动审批资源：{$granted}，现在可以签发 token 了" : '')];
}

/* 网关审计留痕（回环 /admin/audit/record，X-Admin-Key） */
function gw_audit_record(string $user, string $event, string $resource = '-'): void {
  global $gwBase, $gwKey;
  try {
    /* GW-REALIP-20261122：转发用户真实 IP（X-Real-IP），网关审计来源 IP 不再显示 unknown */
    $realIp = (string)($_SERVER['REMOTE_ADDR'] ?? '');
    $ctx = stream_context_create(['http' => [
      'method' => 'POST', 'timeout' => 3,
      'header' => "Content-Type: application/json\r\nX-Admin-Key: " . $gwKey . "\r\nUser-Agent: easyrdp-site-record\r\n"
        . ($realIp !== '' ? "X-Real-IP: " . $realIp . "\r\n" : ""),
      'content' => (string)json_encode(['user' => $user, 'event' => $event, 'resource' => $resource]),
      'ignore_errors' => true,
    ]]);
    @file_get_contents(rtrim($gwBase, '/') . '/admin/audit/record', false, $ctx);
  } catch (Throwable $e) { /* 留痕失败不影响主流程 */ }
}

/* ---------- 用户操作日志（GW-LOG-20261121） ---------- */
function gw_op_log(PDO $db, string $email, string $op, string $detail = ''): void {
  try {
    $st = $db->prepare('SELECT id FROM users WHERE email = ?');
    $st->execute([$email]);
    $uid = $st->fetchColumn();
    $db->prepare('INSERT INTO user_ops(user_id, email, op, detail, ip, ts) VALUES(?,?,?,?,?,?)')
       ->execute([
         $uid === false ? null : (int)$uid,
         $email, $op, $detail,
         (string)($_SERVER['REMOTE_ADDR'] ?? ''),
         gmdate('Y-m-d\TH:i:s\Z'),
       ]);
    // 滚动清理：仅保留最近 50000 条
    $db->exec('DELETE FROM user_ops WHERE id NOT IN
      (SELECT id FROM user_ops ORDER BY id DESC LIMIT 50000)');
  } catch (Throwable $e) { /* 日志失败不影响主流程 */ }
}

$validEmail = fn(string $e): bool => (bool)preg_match('/^[^@\s]{1,64}@[^@\s]{1,255}\.[A-Za-z]{2,24}$/', $e);

require_once __DIR__ . '/../feedback/common.php';

/* ---------- 动作分发 ---------- */
/* GW-MAILVERIFY-20261121：未验证邮箱阻断网关资源使用（函数须在 switch 顶层定义，
   否则其他 case 分支无法看到该声明） */
function require_verified(PDO $db): void {
  $st = $db->prepare('SELECT COALESCE(email_verified,0) FROM users WHERE id = ?');
  $st->execute([(int)$_SESSION['gw_uid']]);
  if ((int)$st->fetchColumn() !== 1) {
    jfail('邮箱未验证：请先查收邮箱验证码完成验证，验证通过后将自动审批网关资源', 403);
  }
}
switch ($action) {
  case 'register': {
    $email = trim((string)($in['email'] ?? ''));
    $pass  = (string)($in['password'] ?? '');
    if (!$validEmail($email)) jfail('邮箱格式不正确');
    if (strlen($pass) < 10) jfail('密码至少 10 位');
    $st = $db->prepare('SELECT id, status FROM users WHERE email = ?');
    $st->execute([$email]);
    $row = $st->fetch();
    if ($row) {
      if (($row['status'] ?? 'active') === 'self_deleted') {
        jfail('该邮箱账号已被用户自行注销，如需恢复请联系官方重新开通', 403);
      }
      jfail('该邮箱已注册，请直接登录');
    }
    $db->prepare('INSERT INTO users(email, pass_hash, created_at) VALUES(?,?,?)')
       ->execute([$email, password_hash($pass, PASSWORD_DEFAULT), gmdate('Y-m-d\TH:i:s\Z')]);
    /* 注意：gw_op_log 会插入 user_ops 使 lastInsertId 偏移，必须先取 uid */
    $newUid = (int)$db->lastInsertId();
    gw_op_log($db, $email, 'register', '注册云网关服务台账号');
    /* GW-AUDITREG-20261122：对齐 EasySSH 口径——注册动作同步写入网关审计环形 */
    gw_audit_record($email, 'register');
    /* GW-MAILVERIFY-20261121：注册即发送邮箱验证码 */
    $mail = gw_send_verify_mail($db, $newUid, $email);
    echo json_encode(['ok' => true,
      'message' => '注册成功，请登录。' . ($mail['ok'] ? '邮箱验证码已发送，请查收邮箱完成验证后即可正常使用。' : $mail['message'])],
      JSON_UNESCAPED_UNICODE);
    exit;
  }

  case 'login': {
    $email = trim((string)($in['email'] ?? ''));
    $pass  = (string)($in['password'] ?? '');
    if (!$validEmail($email) || $pass === '') jfail('请填写邮箱和密码');
    $st = $db->prepare('SELECT id, pass_hash, status, COALESCE(email_verified,0) AS email_verified FROM users WHERE email = ?');
    $st->execute([$email]);
    $row = $st->fetch(PDO::FETCH_ASSOC);
    if (!$row || !password_verify($pass, (string)$row['pass_hash'])) jfail('邮箱或密码不正确', 401);
    if (($row['status'] ?? 'active') === 'self_deleted') {
      jfail('该账号已被用户自行注销，无法登录；如需恢复请联系官方重新开通', 403);
    }
    session_regenerate_id(true);
    $_SESSION['gw_email'] = $email;
    $_SESSION['gw_uid'] = (int)$row['id'];
    gw_op_log($db, $email, 'login', '登录云网关服务台');
    /* GW-AUDITREG-20261122：对齐 EasySSH 口径——登录动作同步写入网关审计环形 */
    gw_audit_record($email, 'login');
    $verified = ((int)($row['email_verified'] ?? 0)) === 1;
    echo json_encode(['ok' => true, 'email' => $email, 'verified' => $verified], JSON_UNESCAPED_UNICODE);
    exit;
  }

  case 'send_verify': {
    /* GW-MAILVERIFY-20261121：重发邮箱验证码（60s 间隔） */
    require_login();
    $r = gw_send_verify_mail($db, (int)$_SESSION['gw_uid'], (string)$_SESSION['gw_email']);
    if (!$r['ok']) jfail($r['message'], 429);
    echo json_encode(['ok' => true, 'message' => $r['message']], JSON_UNESCAPED_UNICODE);
    exit;
  }

  case 'verify': {
    /* GW-MAILVERIFY-20261121：提交验证码 → 通过后自动审批默认资源 */
    require_login();
    $code = trim((string)($in['code'] ?? ''));
    if (!preg_match('/^\d{6}$/', $code)) jfail('验证码为 6 位数字');
    $r = gw_verify_code($db, (int)$_SESSION['gw_uid'], (string)$_SESSION['gw_email'], $code);
    if (!$r['ok']) jfail($r['message'], 400);
    echo json_encode(['ok' => true, 'verified' => true, 'granted' => $r['granted'],
      'message' => $r['message']], JSON_UNESCAPED_UNICODE);
    exit;
  }

  case 'verify_status': {
    /* GW-MAILVERIFY-20261121：查询当前登录用户验证状态（会话恢复用）
       GW-EMAIL-20261122：返回 email 供前端显示"当前登录：xxx@xxx.com 已登录" */
    require_login();
    $st = $db->prepare('SELECT COALESCE(email_verified,0) AS v FROM users WHERE id = ?');
    $st->execute([(int)$_SESSION['gw_uid']]);
    echo json_encode(['ok' => true, 'email' => (string)$_SESSION['gw_email'],
      'verified' => ((int)$st->fetchColumn()) === 1], JSON_UNESCAPED_UNICODE);
    exit;
  }

  case 'logout': {
    /* GW-LOG-20261121：显式退出（前端退出按钮调用；会话销毁 + 日志留痕） */
    $email = (string)($_SESSION['gw_email'] ?? '');
    if ($email !== '') gw_op_log($db, $email, 'logout', '退出云网关服务台登录');
    $_SESSION = [];
    session_destroy();
    echo json_encode(['ok' => true, 'message' => '已退出登录']);
    exit;
  }

  case 'delete_account': {
    /* GW-SELFDEL-20261121：用户自杀注销——仅标注状态 + 清授权 + 销毁会话；
       操作日志与网关审计记录全部保留，并新增 user_self_deleted 审计留痕。
       只有管理员 delete_user 才会真正级联删除日志/审计。 */
    require_login();
    $email = (string)$_SESSION['gw_email'];
    $uid = (int)$_SESSION['gw_uid'];
    $db->prepare("UPDATE users SET status = 'self_deleted' WHERE id = ?")->execute([$uid]);
    $db->prepare('DELETE FROM grants WHERE user_id = ?')->execute([$uid]);
    gw_op_log($db, $email, 'self_delete', '用户自行注销账号（自杀删除）：授权已清除，日志与审计记录保留');
    gw_audit_record($email, 'user_self_deleted');
    $_SESSION = [];
    session_destroy();
    echo json_encode(['ok' => true, 'message' => '账号已注销。历史操作日志与网关审计记录将按规范保留。'], JSON_UNESCAPED_UNICODE);
    exit;
  }

  /* ---------- GW-MAILVERIFY-20261121：未验证邮箱阻断（require_verified 定义见 switch 前） ---------- */
  case 'my_resources': {
    require_login();
    require_verified($db);
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
    require_verified($db);
    $rid = trim((string)($in['resource_id'] ?? ''));
    if ($rid === '' || strlen($rid) > 128) jfail('resource_id 缺失或超长');
    /* TTL-MAX-20261121：用户自定义有效期，白名单钳制；默认 3600（1 小时），
       31536000 表示"长期（约 10 年，用户可感知的无限期）"。 */
    $allowedTtl = [600, 1800, 3600, 21600, 86400, 604800, 2592000, 31536000];
    $ttl = (int)($in['ttl_seconds'] ?? 3600);
    if (!in_array($ttl, $allowedTtl, true)) $ttl = 3600;
    $st = $db->prepare('SELECT label FROM grants WHERE user_id = ? AND resource_id = ?');
    $st->execute([(int)$_SESSION['gw_uid'], $rid]);
    $label = $st->fetchColumn();
    if ($label === false) jfail('该资源未对你开放授权，请联系官方开通', 403);
    /* 内网回环调网关 POST /admin/issue（X-Admin-Key；TTL 白名单内自定义）
       GW-REALIP3-20261122：转发用户真实 IP（X-Real-IP），网关审计不再显示 unknown */
    $realIp = (string)($_SERVER['REMOTE_ADDR'] ?? '');
    $body = json_encode(['sub' => $_SESSION['gw_email'], 'roles' => ['user'], 'ttl_seconds' => $ttl, 'resource' => $rid]);
    $ctx = stream_context_create(['http' => [
      'method'  => 'POST',
      'timeout' => 5,
      'header'  => "Content-Type: application/json\r\nX-Admin-Key: " . $gwKey . "\r\nUser-Agent: easyrdp-site-issue\r\n"
        . ($realIp !== '' ? "X-Real-IP: " . $realIp . "\r\n" : ""),
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
    $out = [
      'ok' => true,
      'token' => (string)$j['token'],
      'resource_id' => $rid,
      'expires_in' => (int)($j['expires_in'] ?? 3600),
      'gateway' => ['host' => $gwHost, 'port' => $gwPort],
      'hint' => '在客户端「网关（可选）」中按以上参数填写，认证方式选 Token（JWT）',
    ];
    /* GW-LOG-20261121：签发留痕（含 token 源文与全部签发参数，供后台日志详情） */
    gw_op_log($db, (string)$_SESSION['gw_email'], 'issue_token', (string)json_encode($out, JSON_UNESCAPED_UNICODE));
    echo json_encode($out, JSON_UNESCAPED_UNICODE);
    exit;
  }

  /* GW-TOKENLIST-20261122：已签发 token 列表（数据源 user_ops.issue_token 留痕），
     返回最近 50 条，含 token 源文 / 资源 / 签发时间 / 剩余有效期，供前端列表展示 */
  case 'my_tokens': {
    require_login();
    require_verified($db);
    $st = $db->prepare("SELECT detail, ts FROM user_ops WHERE email = ? AND op = 'issue_token' ORDER BY id DESC LIMIT 50");
    $st->execute([(string)$_SESSION['gw_email']]);
    $now = time();
    $tokens = [];
    foreach ($st->fetchAll(PDO::FETCH_ASSOC) as $r) {
      $d = json_decode((string)$r['detail'], true);
      if (!is_array($d) || empty($d['token'])) continue;
      $issueTs = strtotime((string)$r['ts']);
      $expiresIn = (int)($d['expires_in'] ?? 3600);
      $exp = $issueTs + $expiresIn;
      $tokens[] = [
        'token'       => (string)$d['token'],
        'resource_id' => (string)($d['resource_id'] ?? ''),
        'gateway'     => [
          'host' => (string)(($d['gateway'] ?? [])['host'] ?? ''),
          'port' => (int)(($d['gateway'] ?? [])['port'] ?? 0),
        ],
        'issued_at'   => (string)$r['ts'],
        'expires_in'  => $expiresIn,
        'expired'     => $exp <= $now,
        'remaining'   => max(0, $exp - $now),
      ];
    }
    echo json_encode(['ok' => true, 'tokens' => $tokens], JSON_UNESCAPED_UNICODE);
    exit;
  }

  case 'revoke': {
    require_login();
    $rid = trim((string)($in['resource_id'] ?? ''));
    if ($rid === '' || strlen($rid) > 128) jfail('resource_id 缺失或超长');
    $db->prepare('DELETE FROM grants WHERE user_id = ? AND resource_id = ?')
       ->execute([(int)$_SESSION['gw_uid'], $rid]);
    gw_op_log($db, (string)$_SESSION['gw_email'], 'revoke', '自助吊销资源授权：' . $rid);
    echo json_encode(['ok' => true, 'message' => '授权已吊销（如需恢复请联系官方重新开通）'], JSON_UNESCAPED_UNICODE);
    exit;
  }

  default:
    jfail('未知 action');
}
