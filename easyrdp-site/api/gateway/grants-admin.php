<?php
/**
 * POST /api/gateway/grants-admin.php —— 网关授权管理（EasyRDP · GW-GRANT-20261121）
 *                                        （GW-DEL/LOG-20261121 扩展：整户授权/取消、
 *                                         删除用户（清审计+邮件通知）、用户操作日志分页）
 *
 * 鉴权：与 audit.php 同一套管理员会话——POST JSON { user, token }，
 *   经 api/feedback/common.php 的 fb_check_token() 校验。
 *
 * 数据库：与 issue.php 共用同一 SQLite（users / grants / user_ops 表），
 *   路径默认 /www/wwwroot/easyrdp-data/users.db，可由 keys.php users_db 覆盖。
 *
 * 资源列表：keys.php 可选 resources 数组（[["id"=>"rdp-win01","label"=>"Windows 桌板"],...]），
 *   缺省为 rdp-win01 / Windows 桌板；行内「授权」按钮使用第一个资源。
 *
 * 动作（JSON body）：
 *   { action:"list",       user, token }                              → { ok, users, grants, resources }
 *   { action:"grant",      user, token, email, resource_id, label? }  → { ok, message }
 *   { action:"revoke",     user, token, user_id, resource_id }        → { ok, message }
 *   { action:"revoke_user",user, token, user_id }                     → { ok, removed }   整户取消授权
 *   { action:"grant_user", user, token, user_id }                     → { ok, message }   按默认资源整户开通
 *   { action:"delete_user",user, token, user_id }                     → { ok, message, removed_audit }
 *       （删除用户+授权+user_ops，回环调网关清空其审计并留 user_deleted 标记，
 *         邮件通知被删用户；不可恢复）
 *   { action:"user_log",   user, token, user_id, page?, pageSize? }   → { ok, items, total, page, pageSize }
 *       （用户操作日志分页：注册/登录/退出/签发/吊销；issue_token 的 detail 为
 *         完整签发 JSON，含 token 源文/网关地址/端口/资源/有效期）
 *   { action:"user_audit", user, token, user_id, page?, pageSize? }   → { ok, items, total, page, pageSize }
 *       （该用户的网关审计事件分页：代理 GET /admin/audit?user=<email>&limit=500）
 *   { action:"admin_ops",  user, token, page?, pageSize? }            → { ok, items, total, page, pageSize }
 *       （管理员操作留痕分页：user_ops 中全部 admin_* 前缀日志，最新在前）
 *   { action:"admin_op_delete", user, token, ids:[..] }               → { ok, removed, message }
 *       （批量删除管理员操作留痕：仅限 admin_* 前缀行，删除动作留 admin_ops_delete 日志）
 */

require __DIR__ . '/../feedback/common.php';

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') fb_fail('仅支持 POST', 405);
$in = json_decode((string)file_get_contents('php://input'), true);
if (!is_array($in)) fb_fail('请求体必须是 JSON');
$user = (string)($in['user'] ?? '');
$token = (string)($in['token'] ?? '');
if (!fb_check_token($user, $token)) fb_fail('未登录或登录已过期，请重新登录', 401);

$keysFile = __DIR__ . '/keys.php';
if (!is_readable($keysFile)) fb_fail('网关配置缺失（api/gateway/keys.php 未放置），请联系管理员', 500);
$keys = json_decode((string)preg_replace('/^<\?php exit\(0\); \?>\s*/', '', (string)file_get_contents($keysFile)), true);
if (!is_array($keys)) fb_fail('网关配置格式异常', 500);
$usersDb = (string)($keys['users_db'] ?? '/www/wwwroot/easyrdp-data/users.db');
$gwBase = isset($keys['gateway_url']) ? rtrim((string)$keys['gateway_url'], '/') : '';
$gwKey = (string)($keys['gateway_admin_key'] ?? '');
/* 资源注册表（keys.php 可覆盖；与网关 config.toml [[resources]] 对齐） */
$resources = $keys['resources'] ?? [['id' => 'rdp-win01', 'label' => 'Windows 桌板']];

try {
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
  $db->exec('CREATE TABLE IF NOT EXISTS user_ops (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      user_id INTEGER,
      email TEXT NOT NULL,
      op TEXT NOT NULL,
      detail TEXT NOT NULL DEFAULT "",
      ip TEXT NOT NULL DEFAULT "",
      ts TEXT NOT NULL)');
} catch (Throwable $e) {
  fb_fail('用户库不可用，请联系管理员', 500);
}

$action = (string)($in['action'] ?? '');
$validEmail = fn(string $e): bool => (bool)preg_match('/^[^@\s]{1,64}@[^@\s]{1,255}\.[A-Za-z]{2,24}$/', $e);
$validRid = fn(string $r): bool => (bool)preg_match('/^[A-Za-z0-9._-]{1,64}$/', $r);

function gw_json_out(array $data): void {
  header('Content-Type: application/json; charset=utf-8');
  header('Cache-Control: no-store');
  echo json_encode($data, JSON_UNESCAPED_UNICODE);
  exit;
}

/* 回环调用网关管理端点（X-Admin-Key） */
function gw_admin_call(string $gwBase, string $gwKey, string $method, string $path, array $body = []): ?array {
  if ($gwBase === '' || $gwKey === '') return null;
  /* GW-REALIP-20261122：转发管理员真实 IP（X-Real-IP），网关审计来源 IP 不再显示 unknown */
  $realIp = (string)($_SERVER['REMOTE_ADDR'] ?? '');
  $ctx = stream_context_create(['http' => [
    'method' => $method, 'timeout' => 5,
    'header' => "Content-Type: application/json\r\nX-Admin-Key: " . $gwKey . "\r\nUser-Agent: easyrdp-grants-admin\r\n"
      . ($realIp !== '' ? "X-Real-IP: " . $realIp . "\r\n" : ""),
    'content' => $body ? (string)json_encode($body) : '',
    'ignore_errors' => true,
  ]]);
  $resp = @file_get_contents($gwBase . $path, false, $ctx);
  if ($resp === false) return null;
  $j = json_decode((string)$resp, true);
  return is_array($j) ? $j : null;
}

/* 按主键取用户，不存在返回 null */
function gw_user(PDO $db, int $uid): ?array {
  $st = $db->prepare('SELECT id, email, created_at, status FROM users WHERE id = ?');
  $st->execute([$uid]);
  $row = $st->fetch(PDO::FETCH_ASSOC);
  if ($row === false) return null;
  $row['status'] = $row['status'] ?? 'active';
  return $row;
}

/* 管理侧操作留痕（操作者 = 官网管理员账号） */
function gw_admin_op(PDO $db, string $operator, string $op, string $detail): void {
  try {
    $db->prepare('INSERT INTO user_ops(user_id, email, op, detail, ip, ts) VALUES(NULL,?,?,?,?,?)')
       ->execute([$operator, $op, $detail, (string)($_SERVER['REMOTE_ADDR'] ?? ''), gmdate('Y-m-d\TH:i:s\Z')]);
  } catch (Throwable $e) { /* 日志失败不影响主流程 */ }
}

switch ($action) {
  case 'list': {
    $users = $db->query("SELECT id, email, created_at, COALESCE(status,'active') AS status, COALESCE(email_verified,0) AS verified FROM users ORDER BY id")->fetchAll(PDO::FETCH_ASSOC);
    $grants = $db->query('SELECT g.user_id, g.resource_id, g.label, g.granted_at, u.email
        FROM grants g LEFT JOIN users u ON u.id = g.user_id ORDER BY g.user_id, g.resource_id')->fetchAll(PDO::FETCH_ASSOC);
    gw_json_out(['ok' => true, 'users' => $users, 'grants' => $grants, 'resources' => $resources]);
  }

  case 'grant': {
    $email = trim((string)($in['email'] ?? ''));
    $rid = trim((string)($in['resource_id'] ?? ''));
    $label = trim((string)($in['label'] ?? ''));
    if (!$validEmail($email)) fb_fail('邮箱格式不正确');
    if (!$validRid($rid)) fb_fail('resource_id 只允许字母/数字/._- 且 ≤64 字符');
    if (mb_strlen($label) > 64) fb_fail('label 过长（≤64 字符）');
    $st = $db->prepare('SELECT id, status FROM users WHERE email = ?');
    $st->execute([$email]);
    $row = $st->fetch(PDO::FETCH_ASSOC);
    if ($row === false) fb_fail('该邮箱尚未在网关服务台注册', 404);
    if (($row['status'] ?? 'active') === 'self_deleted') fb_fail('该用户已自行注销账号，无法授权', 409);
    $uid = (int)$row['id'];
    $ins = $db->prepare('INSERT OR IGNORE INTO grants(user_id, resource_id, label, granted_at) VALUES(?,?,?,?)');
    $ins->execute([(int)$uid, $rid, $label, gmdate('Y-m-d\TH:i:s\Z')]);
    if ($ins->rowCount() === 0) {
      $db->prepare('UPDATE grants SET label = ? WHERE user_id = ? AND resource_id = ?')
         ->execute([$label, (int)$uid, $rid]);
    }
    gw_admin_op($db, $user, 'admin_grant', "管理员开通授权：$email → $rid");
    gw_json_out(['ok' => true, 'message' => '已授权：' . $email . ' → ' . $rid]);
  }

  case 'revoke': {
    $uid = (int)($in['user_id'] ?? 0);
    $rid = trim((string)($in['resource_id'] ?? ''));
    if ($uid <= 0 || !$validRid($rid)) fb_fail('参数不合法');
    $db->prepare('DELETE FROM grants WHERE user_id = ? AND resource_id = ?')->execute([$uid, $rid]);
    gw_admin_op($db, $user, 'admin_revoke', "管理员吊销授权：user_id=$uid, $rid");
    gw_json_out(['ok' => true, 'message' => '已吊销授权']);
  }

  case 'revoke_user': {
    /* GW-DEL-20261121：整户取消授权（保留账号与日志，仅清 grants） */
    $uid = (int)($in['user_id'] ?? 0);
    $u = gw_user($db, $uid);
    if (!$u) fb_fail('用户不存在', 404);
    $st = $db->prepare('DELETE FROM grants WHERE user_id = ?');
    $st->execute([$uid]);
    gw_admin_op($db, $user, 'admin_revoke_user', "管理员整户取消授权：{$u['email']}（{$st->rowCount()} 项）");
    gw_json_out(['ok' => true, 'removed' => $st->rowCount(),
      'message' => '已取消 ' . $u['email'] . ' 的全部授权（' . $st->rowCount() . ' 项）']);
  }

  case 'grant_user': {
    /* GW-DEL-20261121：整户开通授权（使用资源列表第一项作为默认资源） */
    $uid = (int)($in['user_id'] ?? 0);
    $u = gw_user($db, $uid);
    if (!$u) fb_fail('用户不存在', 404);
    if (($u['status'] ?? 'active') === 'self_deleted') fb_fail('该用户已自行注销账号，无法授权', 409);
    if (empty($resources) || !is_array($resources)) fb_fail('资源列表未配置', 500);
    $res0 = $resources[0];
    $rid = (string)($res0['id'] ?? '');
    $label = (string)($res0['label'] ?? '');
    if (!$validRid($rid)) fb_fail('默认资源 ID 未配置', 500);
    $db->prepare('INSERT OR IGNORE INTO grants(user_id, resource_id, label, granted_at) VALUES(?,?,?,?)')
       ->execute([$uid, $rid, $label, gmdate('Y-m-d\TH:i:s\Z')]);
    gw_admin_op($db, $user, 'admin_grant_user', "管理员整户开通授权：{$u['email']} → $rid");
    gw_json_out(['ok' => true, 'message' => '已授权：' . $u['email'] . ' → ' . $rid]);
  }

  case 'delete_user': {
    /* GW-DEL-20261121：彻底删除用户——users + grants + user_ops 全清，
       回环调网关 /admin/audit/purge_user 清空其审计并留 user_deleted 标记，
       邮件通知被删用户。不可恢复，前端需二次确认。 */
    $uid = (int)($in['user_id'] ?? 0);
    $u = gw_user($db, $uid);
    if (!$u) fb_fail('用户不存在', 404);
    $email = (string)$u['email'];
    // 1) 清空网关审计关联记录（含 user_deleted 留痕）
    $purge = gw_admin_call($gwBase, $gwKey, 'POST', '/admin/audit/purge_user', ['user' => $email]);
    $removedAudit = is_array($purge) ? (int)($purge['removed'] ?? 0) : -1;
    // 2) 删除本地数据
    $db->prepare('DELETE FROM grants WHERE user_id = ?')->execute([$uid]);
    $db->prepare('DELETE FROM user_ops WHERE email = ? OR user_id = ?')->execute([$email, $uid]);
    $db->prepare('DELETE FROM users WHERE id = ?')->execute([$uid]);
    // 3) 留一条管理员删除日志（不关联被删用户，避免再次被级联清掉）
    //    注意：PHP 双引号内 $var 后接全角字符会被整体解析为长变量名，必须用 {$var}
    gw_admin_op($db, $user, 'admin_delete_user',
      "管理员删除用户：{$email}（网关审计清除 {$removedAudit} 条，注册于 {$u['created_at']}）");
    // 4) 邮件通知被删用户
    $mailSent = fb_mail_send($email,
      'EasyRDP 云网关账号已删除',
      "您好：\n\n您在 EasyRDP 云网关用户服务台（https://easyrdp.cn/gw-admin.html）的账号（{$email}）"
      . "已被管理员删除，关联授权与操作记录已一并清除。\n"
      . "如有疑问请通过 EasyRDP 官网反馈渠道联系我们。\n\n"
      . "—— EasyRDP 云网关\n" . gmdate('Y-m-d\TH:i:s\Z'));
    gw_json_out(['ok' => true, 'removed_audit' => $removedAudit, 'mail' => $mailSent === true,
      'message' => '已删除 ' . $email . '（网关审计清除 ' . ($removedAudit < 0 ? '失败/不可达' : $removedAudit . ' 条') . '，邮件' . ($mailSent === true ? '已发送' : '发送失败：' . $mailSent) . '）']);
  }

  case 'admin_ops': {
    /* GW-AUDIT-DELOP-20261122：管理员操作留痕查看（user_ops 中全部 admin_* 级联日志，
       含 admin_audit_delete / admin_delete_user / admin_grant 等，永久留存、最新在前分页） */
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = (int)($in['pageSize'] ?? 20);
    if ($pageSize < 1 || $pageSize > 100) $pageSize = 20;
    $st = $db->query("SELECT COUNT(*) FROM user_ops WHERE substr(op,1,6) = 'admin_'");
    $total = (int)$st->fetchColumn();
    $st = $db->prepare('SELECT id, email, op, detail, ip, ts FROM user_ops
        WHERE substr(op,1,6) = \'admin_\' ORDER BY id DESC LIMIT ? OFFSET ?');
    $st->execute([$pageSize, ($page - 1) * $pageSize]);
    gw_json_out(['ok' => true, 'items' => $st->fetchAll(PDO::FETCH_ASSOC),
      'total' => $total, 'page' => $page, 'pageSize' => $pageSize]);
  }

  case 'admin_op_delete': {
    /* GW-OPSMODAL-20261122：批量删除管理员操作留痕（仅限 admin_* 前缀行，按 id 白名单）。
       用户侧日志（register/login/issue_token 等）不在本动作权限内；
       删除动作本身写一条 admin_ops_delete 留痕（与 GW-AUDIT-DELOP 同规范）。 */
    $ids = $in['ids'] ?? [];
    if (!is_array($ids) || !count($ids) || count($ids) > 500) fb_fail('ids 为空或超限（1-500）');
    $clean = [];
    foreach ($ids as $id) {
      if (!is_numeric($id) || (int)$id <= 0) fb_fail('存在不合法的日志 id');
      $clean[] = (int)$id;
    }
    $ph = implode(',', array_fill(0, count($clean), '?'));
    $st = $db->prepare("DELETE FROM user_ops WHERE id IN ($ph) AND substr(op,1,6) = 'admin_'");
    $st->execute($clean);
    $removed = $st->rowCount();
    gw_admin_op($db, $user, 'admin_ops_delete', "管理员删除管理员操作日志：成功 {$removed} / 请求 " . count($clean) . ' 条');
    gw_json_out(['ok' => true, 'removed' => $removed,
      'message' => '已删除 ' . $removed . ' 条管理员操作日志']);
  }

  case 'user_log': {
    /* GW-LOG-20261121：用户操作日志分页 */
    $uid = (int)($in['user_id'] ?? 0);
    $u = gw_user($db, $uid);
    if (!$u) fb_fail('用户不存在', 404);
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = (int)($in['pageSize'] ?? 10);
    if ($pageSize < 1 || $pageSize > 100) $pageSize = 10;
    $st = $db->prepare('SELECT COUNT(*) FROM user_ops WHERE email = ? OR user_id = ?');
    $st->execute([$u['email'], $uid]);
    $total = (int)$st->fetchColumn();
    $st = $db->prepare('SELECT id, op, detail, ip, ts FROM user_ops
        WHERE email = ? OR user_id = ? ORDER BY id DESC LIMIT ? OFFSET ?');
    $st->execute([$u['email'], $uid, $pageSize, ($page - 1) * $pageSize]);
    gw_json_out(['ok' => true, 'items' => $st->fetchAll(PDO::FETCH_ASSOC),
      'total' => $total, 'page' => $page, 'pageSize' => $pageSize]);
  }

  case 'user_audit': {
    /* GW-LOG-20261121：该用户的网关审计事件分页（代理 /admin/audit?user=email&limit=500） */
    $uid = (int)($in['user_id'] ?? 0);
    $u = gw_user($db, $uid);
    if (!$u) fb_fail('用户不存在', 404);
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = (int)($in['pageSize'] ?? 10);
    if ($pageSize < 1 || $pageSize > 100) $pageSize = 10;
    $j = gw_admin_call($gwBase, $gwKey, 'GET', '/admin/audit?user=' . urlencode((string)$u['email']) . '&limit=500');
    if (!is_array($j) || ($j['ok'] ?? false) !== true || !isset($j['events'])) {
      fb_fail('网关不可达或审计读取失败，请稍后重试', 502);
    }
    $events = array_values($j['events']); // 网关已按最新在前排序
    $total = count($events);
    $items = array_slice($events, ($page - 1) * $pageSize, $pageSize);
    gw_json_out(['ok' => true, 'items' => $items, 'total' => $total, 'page' => $page, 'pageSize' => $pageSize]);
  }

  case 'delete_ops': {
    /* GWLOG-DEL-20261121：批量删除该用户的操作日志（按 id 白名单校验归属） */
    $uid = (int)($in['user_id'] ?? 0);
    $u = gw_user($db, $uid);
    if (!$u) fb_fail('用户不存在', 404);
    $ids = $in['ids'] ?? [];
    if (!is_array($ids) || !count($ids) || count($ids) > 500) fb_fail('ids 为空或超限（1-500）');
    $clean = [];
    foreach ($ids as $id) {
      if (!is_numeric($id) || (int)$id <= 0) fb_fail('存在不合法的日志 id');
      $clean[] = (int)$id;
    }
    $ph = implode(',', array_fill(0, count($clean), '?'));
    $st = $db->prepare("DELETE FROM user_ops WHERE id IN ($ph) AND (email = ? OR user_id = ?)");
    $st->execute(array_merge($clean, [$u['email'], $uid]));
    gw_admin_op($db, $user, 'admin_delete_ops', "管理员删除操作日志：{$u['email']}（{$st->rowCount()} 条）");
    gw_json_out(['ok' => true, 'removed' => $st->rowCount(),
      'message' => '已删除 ' . $st->rowCount() . ' 条操作日志']);
  }

  default:
    fb_fail('未知 action');
}
