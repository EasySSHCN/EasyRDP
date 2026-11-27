<?php
/**
 * GET/POST /api/download/ticket.php —— 下载票据签发（官网访客-设备归因链路 · R-20261118-001）
 *
 * 流程：用户点击官网 download.html 下载按钮 → 前端 JS 先请求本接口 → 签发票据并落盘
 *       → 前端拿到 ticket_id 后再触发原下载（失败时前端静默降级直下，不影响下载）。
 *
 * 票据记录 = { <ticket_id>: {
 *   ip(完整IP), ipm(IP掩码), uv(当日UV哈希前12位), ua(浏览器家族),
 *   ref(来源主机名), rf(完整来源URL), visitId(关联访客流水ID,可为空),
 *   ts(签发毫秒时间戳), expire(归因匹配窗截止), attributed:[](已归因设备anonId,上限10台)
 * }}
 * 归因窗 7 天：客户端首报携带 ticket_id → api/stats/client.php report 在票据上标记
 *   「已归因设备」并在设备档案写 origin{via:'ticket',visitId} → visit.php produced /
 *   detail 的 devices 反向标注自动生效（admin.html「已产生设备的访客记录」出现数据）。
 * 票据保留 90 天后清理；同 IP 每小时签发上限 30 次（滑动窗口频控）。
 *
 * 隐私：ticket_id 为纯随机数，不含任何个人身份信息；归因建立的是
 *   「匿名访问 ↔ 匿名设备」的弱关联（对齐 EasySSH site/api/download/ticket.php 同族实现）。
 * 存储：/www/wwwroot/easyrdp-data/tickets/（webroot 之外，HTTP 不可直达；
 *   flock + tmp/rename 原子写，复用 api/stats/common.php 的 st_locked_update）。
 */

require __DIR__ . '/../stats/common.php';

define('TK_DATA_DIR', dirname(__DIR__, 3) . '/easyrdp-data/tickets');
define('TK_FILE', TK_DATA_DIR . '/tickets.json');
define('TK_RATE_FILE', TK_DATA_DIR . '/tickets-ratelimit.json');
define('TK_MATCH_WINDOW_MS', 7 * 24 * 3600 * 1000);  // 归因匹配窗（客户端首报有效窗口）
define('TK_KEEP_MS', 90 * 24 * 3600 * 1000);         // 票据记录保留期（90 天）
define('TK_MAX', 5000);                              // 票据条数上限（防灌水）
define('TK_RATE_PER_HOUR', 30);                      // 同 IP 每小时签发上限
define('TK_VISIT_LOOKBACK_MS', 10 * 60 * 1000);      // 签发时向前回溯访客流水的时间窗
define('TK_ATTRIBUTED_MAX', 10);                     // 单票可归因设备数上限（绿色版复制场景）

header('Cache-Control: no-store');

$now = st_now_ms();
$ip = fb_client_ip();
$ua = (string)($_SERVER['HTTP_USER_AGENT'] ?? '');

// 同 IP 频控（滑动小时窗口，复用 stats 公共频控；超限静默 429）
if (!st_rate_ok(TK_RATE_FILE, $ip, TK_RATE_PER_HOUR)) {
  st_fail('请求过于频繁', 429);
}

// ---- 关联访客流水：找「同完整 IP + 10 分钟内」最近一次访问（recent 最新在前）----
$visitId = '';
$refFull = '';
$vdb = st_db_load(ST_VISIT_DB, 'visit');
if (isset($vdb['recent']) && is_array($vdb['recent'])) {
  foreach ($vdb['recent'] as $r) {
    if (!is_array($r)) continue;
    if ((string)($r['f'] ?? '') !== (string)$ip) continue;
    if ($now - (int)($r['t'] ?? 0) > TK_VISIT_LOOKBACK_MS) break; // 超窗即可停止
    $visitId = (string)($r['id'] ?? '');
    $refFull = (string)($r['rf'] ?? '');
    break;
  }
}
$ref = $refFull !== '' ? st_ref_of($refFull) : st_ref_of((string)($_GET['r'] ?? ($_POST['r'] ?? '')));

// ---- 票据入库（flock 内过期清理 + 上限裁剪 + 原子写）----
$ticketId = bin2hex(random_bytes(16)); // 32 位随机 hex，与 anonId 同字符集
$rec = [
  'ip' => (string)$ip,
  'ipm' => st_ip_mask($ip),
  'uv' => substr(st_uv_hash($ip, $ua), 0, 12),
  'ua' => st_ua_family($ua),
  'ref' => $ref,
  'rf' => $refFull,
  'visitId' => $visitId,
  'ts' => $now,
  'expire' => $now + TK_MATCH_WINDOW_MS,
  'attributed' => [],
];
$okTicket = st_locked_update(TK_FILE, 'ticket', function ($db) use ($ticketId, $rec, $now) {
  if (!isset($db['tickets']) || !is_array($db['tickets'])) $db = ['tickets' => [], 'meta' => ['created' => $now]];
  $tickets = &$db['tickets'];
  foreach ($tickets as $k => $t) {
    if (!is_array($t) || (int)($t['ts'] ?? 0) < $now - TK_KEEP_MS) unset($tickets[$k]);
  }
  if (count($tickets) >= TK_MAX) {
    uasort($tickets, function ($a, $b) { return (int)($a['ts'] ?? 0) <=> (int)($b['ts'] ?? 0); });
    $tickets = array_slice($tickets, (int)(TK_MAX / 4), null, true);
  }
  unset($tickets);
  $db['tickets'][$ticketId] = $rec;
  $db['meta']['updated'] = $now;
  $db['meta']['count'] = count($db['tickets']);
  return $db;
});

// 票据落盘失败（极端磁盘故障）：返回失败，前端静默降级直下，不影响下载主流程
if ($okTicket === null) st_fail('票据签发失败，请直接下载', 500);

st_json_out(['ok' => true, 'ticket_id' => $ticketId, 'src' => $ref]);
