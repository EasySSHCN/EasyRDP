<?php
/**
 * POST /api/stats/updates.php —— 官网更新记录（EasyRDP 官网后台 · 🚀官网更新记录标签页）
 *
 * 数据源 = api/update/check.php 在每次客户端更新检测时轻量追加的流水
 * （/www/wwwroot/easyrdp-data/stats/updates-log.json，事件：check / update_available）。
 * 本接口只读聚合 + 分页；鉴权复用反馈管理员令牌（user+token）。
 *
 * 动作：
 *   { action:"summary", user, token }                    → { ok, summary:{...} }
 *   { action:"list",    user, token, page?, pageSize?, type? } → { ok, rows, total }
 *   { action:"purge",   user, token }                    → { ok } 清空记录流水
 */
require __DIR__ . '/common.php';

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') st_fail('仅支持 POST', 405);
$in = json_decode(file_get_contents('php://input'), true);
if (!is_array($in)) st_fail('请求体必须是 JSON');
$action = (string)($in['action'] ?? '');
if (!fb_check_token((string)($in['user'] ?? ''), (string)($in['token'] ?? ''))) {
  st_fail('未登录或登录已过期，请重新登录', 401);
}

$db = st_db_load(ST_UPDATES_LOG, 'updates');
$events = array_values(array_filter(is_array($db['events'] ?? null) ? $db['events'] : [], 'is_array'));

switch ($action) {
  case 'summary': {
    $today = date('Y-m-d');
    $weekStart = date('Y-m-d', strtotime('monday this week'));
    $nowMs = st_now_ms();
    $todayC = 0; $weekC = 0; $byDay = []; $byVer = []; $avail = 0;
    foreach ($events as $e) {
      $d = (string)($e['day'] ?? date('Y-m-d', (int)(($e['t'] ?? 0) / 1000)));
      if ($d === $today) $todayC++;
      if ($d >= $weekStart) $weekC++;
      $byDay[$d] = ($byDay[$d] ?? 0) + 1;
      $kind = (string)($e['kind'] ?? '');
      if ($kind === 'update_available') {
        $avail++;
        $v = trim((string)($e['latest'] ?? ''));
        if ($v !== '') $byVer[$v] = ($byVer[$v] ?? 0) + 1;
      }
    }
    arsort($byVer);
    $trend = [];
    for ($i = 29; $i >= 0; $i--) {
      $d = date('Y-m-d', (int)(($nowMs - $i * 24 * 3600 * 1000) / 1000));
      $trend[] = ['date' => $d, 'count' => (int)($byDay[$d] ?? 0)];
    }
    st_json_out(['ok' => true, 'summary' => [
      'total' => count($events), 'today' => $todayC, 'week' => $weekC,
      'updateAvailable' => $avail,
      'byVersion' => array_slice($byVer, 0, 10, true),
      'trend' => $trend,
    ]]);
  }

  case 'list': {
    $type = trim((string)($in['type'] ?? ''));
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = min(200, max(1, (int)($in['pageSize'] ?? 20)));
    $rows = array_reverse($events); // 最新在前
    if ($type !== '') $rows = array_values(array_filter($rows, function ($e) use ($type) {
      return (string)($e['kind'] ?? '') === $type;
    }));
    $total = count($rows);
    st_json_out(['ok' => true, 'rows' => array_slice($rows, ($page - 1) * $pageSize, $pageSize), 'total' => $total]);
  }

  case 'purge':
    st_db_save(ST_UPDATES_LOG, st_db_default('updates'));
    st_json_out(['ok' => true]);

  default:
    st_fail('未知动作');
}
