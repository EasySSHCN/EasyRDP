<?php
/**
 * POST /api/gateway/audit.php —— 网关审计代理（EasyRDP · 🛡网关审计标签页）
 * （F5 管理控制台第一步：把 easyrdp-gateway 审计日志经受保护 API 开放到官网后台）
 *
 * 鉴权：与 api/stats/admin.php 同一套会话模式——POST JSON { user, token }，
 *   经 api/feedback/common.php 的 fb_check_token() 校验管理员登录会话。
 *
 * 配置：__DIR__/keys.php（参照 keys.example.php 模板人工放置，真实密钥绝不入库）：
 *   gateway_url        网关地址（如 http://127.0.0.1:8443）
 *   gateway_admin_key   与 gateway config.toml [admin].admin_key 一致的共享密钥
 *
 * 动作（JSON body）：
 *   { action:"audit", user, token, limit? }  → { ok, count, events:[{timestamp,event,source_ip,resource,result}] }
 *     limit 仅接受 50/100/200（默认 100，其余值回退 100）；网关侧再做 1..=500 钳制。
 *
 * 错误结构（网关不可达 / 密钥无效 / 配置缺失时返回明确 JSON，HTTP 状态对应）：
 *   { ok:false, message:"..." }
 *   配置缺失 → 500；会话无效 → 401；网关不可达 → 502；网关拒绝（密钥错/禁用）→ 502（透传 message）。
 */

require __DIR__ . '/../feedback/common.php';

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') fb_fail('仅支持 POST', 405);
$in = json_decode((string)file_get_contents('php://input'), true);
if (!is_array($in)) fb_fail('请求体必须是 JSON');
$user = (string)($in['user'] ?? '');
$token = (string)($in['token'] ?? '');
if (!fb_check_token($user, $token)) fb_fail('未登录或登录已过期，请重新登录', 401);

/* ---------- 加载网关配置（keys.php 人工放置；缺失 = 未部署） ---------- */
$keysFile = __DIR__ . '/keys.php';
if (!is_readable($keysFile)) fb_fail('网关配置缺失（api/gateway/keys.php 未放置），请联系管理员', 500);
$keys = json_decode((string)preg_replace('/^<\?php exit\(0\); \?>\s*/', '', (string)file_get_contents($keysFile)), true);
if (!is_array($keys) || empty($keys['gateway_url']) || empty($keys['gateway_admin_key'])) {
  fb_fail('网关配置不完整（gateway_url / gateway_admin_key 缺失）', 500);
}
$gwBase = rtrim((string)$keys['gateway_url'], '/');
$gwKey = (string)$keys['gateway_admin_key'];

/* ---------- limit 白名单（50/100/200，默认 100） ---------- */
$limit = (int)($in['limit'] ?? 100);
if (!in_array($limit, [50, 100, 200], true)) $limit = 100;

/* ---------- GW-AUDIT-DEL-20261121：action=delete 批量删除审计事件 ---------- */
$action = (string)($in['action'] ?? 'audit');
if ($action === 'delete') {
  $items = $in['items'] ?? [];
  if (!is_array($items) || !count($items) || count($items) > 500) fb_fail('items 为空或超限（1-500）');
  $clean = [];
  foreach ($items as $it) {
    if (!is_array($it)) fb_fail('items 格式不正确');
    $ts = $it['timestamp'] ?? null;
    $u = trim((string)($it['user'] ?? ''));
    $ev = trim((string)($it['event'] ?? ''));
    if (!is_numeric($ts) || $u === '' || $ev === '' || mb_strlen($u) > 64 || mb_strlen($ev) > 64) {
      fb_fail('存在不合法的审计条目');
    }
    $clean[] = ['timestamp' => (int)$ts, 'user' => $u, 'event' => $ev];
  }
  $ctx = stream_context_create(['http' => [
    'method'  => 'POST',
    'timeout' => 5,
    'header'  => "Content-Type: application/json\r\nX-Admin-Key: " . $gwKey . "\r\nUser-Agent: easyrdp-site-audit-del\r\n"
      . ((string)($_SERVER['REMOTE_ADDR'] ?? '') !== '' ? "X-Real-IP: " . (string)$_SERVER['REMOTE_ADDR'] . "\r\n" : ""),
    'content' => (string)json_encode(['items' => $clean]),
    'ignore_errors' => true,
  ]]);
  $resp = @file_get_contents($gwBase . '/admin/audit/delete_batch', false, $ctx);
  if ($resp === false) fb_fail('网关不可达（gateway 连接失败或超时），请稍后重试', 502);
  $j = json_decode((string)$resp, true);
  if (!is_array($j) || ($j['ok'] ?? false) !== true) {
    $msg = is_array($j) && isset($j['message']) ? (string)$j['message'] : '网关删除失败';
    fb_fail('网关返回错误：' . $msg, 502);
  }
  /* GW-AUDIT-DELOP-20261122：批量删除审计事件 → 永久管理员操作留痕（users.db user_ops）。
     审计环本身保持干净（不自指回写）；追溯走这里 + 网关 journal。 */
  try {
    $usersDb = (string)($keys['users_db'] ?? '/www/wwwroot/easyrdp-data/users.db');
    $dbDir = dirname($usersDb);
    if (!is_dir($dbDir)) @mkdir($dbDir, 0750, true);
    $opDb = new PDO('sqlite:' . $usersDb, null, null, [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]);
    $opDb->exec('CREATE TABLE IF NOT EXISTS user_ops (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id INTEGER,
        email TEXT NOT NULL,
        op TEXT NOT NULL,
        detail TEXT NOT NULL DEFAULT "",
        ip TEXT NOT NULL DEFAULT "",
        ts TEXT NOT NULL)');
    $removed = (int)($j['removed'] ?? 0);
    $summary = [];
    foreach (array_slice($clean, 0, 20) as $it) {
      $summary[] = $it['event'] . '@' . $it['user'];
    }
    $more = count($clean) > 20 ? ' 等 ' . count($clean) . ' 条' : '';
    $opDb->prepare('INSERT INTO user_ops(user_id, email, op, detail, ip, ts) VALUES(NULL,?,?,?,?,?)')
       ->execute([
         $user,
         'admin_audit_delete',
         '管理员删除网关审计记录：成功 ' . $removed . ' / 请求 ' . count($clean)
           . ' 条（' . implode('、', $summary) . $more . '）',
         (string)($_SERVER['REMOTE_ADDR'] ?? ''),
         gmdate('Y-m-d\TH:i:s\Z'),
       ]);
  } catch (Throwable $e) { /* 留痕失败不影响删除主流程 */ }
  header('Content-Type: application/json; charset=utf-8');
  header('Cache-Control: no-store');
  echo json_encode(['ok' => true, 'removed' => (int)($j['removed'] ?? 0),
    'message' => (string)($j['message'] ?? '已删除')], JSON_UNESCAPED_UNICODE);
  exit;
}

/* ---------- 服务端代理请求网关 GET /admin/audit?limit=N ---------- */
$url = $gwBase . '/admin/audit?limit=' . $limit;
$ctx = stream_context_create(['http' => [
  'method'  => 'GET',
  'timeout' => 5,
  'header'  => "X-Admin-Key: " . $gwKey . "\r\nUser-Agent: easyrdp-site-audit-proxy\r\n",
  'ignore_errors' => true, // 4xx/5xx 也要读到响应体以透传原因
]]);
$resp = @file_get_contents($url, false, $ctx);
if ($resp === false) {
  fb_fail('网关不可达（gateway 连接失败或超时），请稍后重试', 502);
}
// $http_response_header 形如 ["HTTP/1.1 200 OK", ...]；取第一个状态行
$status = 0;
foreach (($http_response_header ?? []) as $h) {
  if (preg_match('#^HTTP/\S+\s+(\d{3})#', $h, $m)) { $status = (int)$m[1]; break; }
}
$j = json_decode((string)$resp, true);
if (!is_array($j) || ($j['ok'] ?? false) !== true || !isset($j['events']) || !is_array($j['events'])) {
  $msg = is_array($j) && isset($j['message']) ? (string)$j['message'] : '网关响应格式异常';
  fb_fail('网关返回错误（HTTP ' . ($status ?: '?') . '）：' . $msg, 502);
}

header('Content-Type: application/json; charset=utf-8');
header('Cache-Control: no-store');
echo json_encode(['ok' => true, 'count' => (int)($j['count'] ?? count($j['events'])),
  'events' => array_values($j['events'])], JSON_UNESCAPED_UNICODE);
exit;
