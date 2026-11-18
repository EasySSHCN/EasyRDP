<?php
/**
 * POST /api/stats/client.php —— 客户端统计（EasyRDP 官网后台 · 📊客户端统计标签页数据源）
 *
 * 数据来源（聚合）：
 *   1) 遥测上报（预留）：action=report，客户端匿名上报 { anonId, fields:{} }（白名单字段，
 *      值截断；同 IP 每小时 30 次频控；IP 只存掩码/哈希不存明文）。
 *   2) 反馈系统信息（只读聚合）：api/feedback 反馈记录中的 systemInfo 字段，
 *      每条反馈按「一次提交」聚合为一条伪设备记录（anonId = "fb:<反馈id>"），与遥测设备并表。
 *
 * 管理动作（JSON body，复用反馈管理员令牌鉴权 user+token）：
 *   { action:"summary",     user, token }                              → { ok, summary:{...看板聚合...} }
 *   { action:"list",        user, token, page?, pageSize? }            → { ok, rows, total }  设备列表
 *   { action:"log",         user, token, page?, pageSize? }            → { ok, rows, total }  提交流水
 *   { action:"bucket_list", user, token, bucket, value, page?, pageSize? } → { ok, rows, total } 分布明细
 *   { action:"purge",       user, token }                              → { ok } 清空遥测库（反馈聚合不受影响）
 *
 * 公开动作（无需登录）：
 *   { action:"report", anonId, fields:{} } → { ok:true }
 *   { action:"report", anonId, ticket_id?, fields:{} } → { ok:true }
 *     R-20261118-001 归因：ticket_id 为官网下载票据（api/download/ticket.php 签发），
 *     有效票据（7 天窗内）在票据上标记「已归因设备」并在设备档案写
 *     origin{via:'ticket',ticketId,visitId} → visit.php produced 的 devices 反向标注生效。
 *     同一设备每日仅首报计一次（activity 按日去重），归因只在首报时建立。
 */
require __DIR__ . '/common.php';

if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') st_fail('仅支持 POST', 405);
$in = json_decode(file_get_contents('php://input'), true);
if (!is_array($in)) st_fail('请求体必须是 JSON');
$action = (string)($in['action'] ?? '');

/* ---------- 公开：客户端遥测上报（预留入口） ---------- */
if ($action === 'report') {
  $ip = fb_client_ip();
  if (!st_rate_ok(ST_RATE_FILE, $ip, ST_RATE_PER_HOUR)) st_fail('上报过于频繁', 429);
  $anonId = (string)($in['anonId'] ?? '');
  if (!st_valid_anon_id($anonId)) st_fail('anonId 非法');
  // 下载票据（可选）：32 位随机 hex，字符集校验与 anonId 一致；非法值直接忽略
  $ticketId = trim((string)($in['ticket_id'] ?? ''));
  if ($ticketId !== '' && !st_valid_anon_id($ticketId)) $ticketId = '';
  // R-20261118-001 归因票据存储（与 api/download/ticket.php 同一路径约定；本文件不加载 ticket.php）
  if (!defined('ST_TICKET_DB')) {
    define('ST_TICKET_DB', dirname(__DIR__, 3) . '/easyrdp-data/tickets/tickets.json');
    define('ST_TICKET_WINDOW_MS', 7 * 24 * 3600 * 1000);
    define('ST_TICKET_ATTRIBUTED_MAX', 10);
  }
  // 有效票据 = 存在且在 7 天归因窗内
  $ticket = null;
  if ($ticketId !== '') {
    $tdb = st_db_load(ST_TICKET_DB, 'ticket');
    $t = isset($tdb['tickets'][$ticketId]) && is_array($tdb['tickets'][$ticketId]) ? $tdb['tickets'][$ticketId] : null;
    if ($t !== null && st_now_ms() - (int)($t['ts'] ?? 0) <= ST_TICKET_WINDOW_MS) $ticket = $t;
  }
  $fields = st_sanitize(is_array($in['fields'] ?? null) ? $in['fields'] : []);
  $now = st_now_ms();
  $d = date('Y-m-d');
  // GEO-20261118：地域 = 中文归属地名（查询链+前缀缓存）+ 掩码前缀，对齐 EasySSH st_apply_region 语义
  $pp = st_ip_prefix($ip);
  if ($pp !== null) {
    $geoName = st_region($ip);
    $fields['地域'] = trim(($geoName !== '' ? $geoName . ' ' : '') . st_ip_mask_label($pp[0], $pp[1]));
  }
  // $dup：当日已上报（频控按日去重，重复上报只刷新 lastSeen）；$dupOut 供票据归因判断
  $dupOut = false;
  st_locked_update(ST_CLIENT_DB, 'client', function ($db) use ($anonId, $fields, $now, $d, $ticket, $ticketId, &$dupOut) {
    if (!isset($db['clients'][$anonId]) || !is_array($db['clients'][$anonId])) {
      $db['clients'][$anonId] = ['firstSeen' => $now, 'lastSeen' => $now, 'reports' => 0, 'fields' => []];
    }
    $c = &$db['clients'][$anonId];
    $c['lastSeen'] = $now;
    $dup = isset($db['activity'][$anonId]) && is_array($db['activity'][$anonId])
      && isset($db['activity'][$anonId][$d]);
    $dupOut = $dup;
    // 有效票据 → 设备档案写归因来源（visit.php st_attach_devices 读 origin.visitId 反向标注）
    if ($ticket !== null) {
      $c['origin'] = ['via' => 'ticket', 'ticketId' => $ticketId,
        'visitId' => (string)($ticket['visitId'] ?? ''), 'ts' => $now];
    }
    if (!$dup) {
      $c['reports'] = (int)($c['reports'] ?? 0) + 1;
      $c['fields'] = array_merge(is_array($c['fields'] ?? null) ? $c['fields'] : [], $fields);
      if (!isset($db['activity'][$anonId]) || !is_array($db['activity'][$anonId])) $db['activity'][$anonId] = [];
      $db['activity'][$anonId][$d] = (int)($db['activity'][$anonId][$d] ?? 0) + 1;
      $db['log'][] = ['t' => $now, 'id' => $anonId, 'v' => (string)($fields['软件版本'] ?? '')];
      if (count($db['log']) > ST_LOG_MAX) $db['log'] = array_slice($db['log'], -ST_LOG_MAX);
    }
    unset($c);
    return $db;
  });
  // 票据归因标记（首报才标记；flock 原子写，单票设备数上限防灌水）
  if ($ticket !== null && !$dupOut) {
    st_locked_update(ST_TICKET_DB, 'ticket', function ($db) use ($anonId, $ticketId, $now) {
      if (!isset($db['tickets']) || !is_array($db['tickets'])) return $db;
      if (!isset($db['tickets'][$ticketId]) || !is_array($db['tickets'][$ticketId])) return $db;
      $t = &$db['tickets'][$ticketId];
      if (!isset($t['attributed']) || !is_array($t['attributed'])) $t['attributed'] = [];
      if (in_array($anonId, $t['attributed'], true)) return $db;
      if (count($t['attributed']) >= ST_TICKET_ATTRIBUTED_MAX) return $db;
      $t['attributed'][] = $anonId;
      $t['attributedAt'] = $now;
      unset($t);
      return $db;
    });
  }
  st_json_out(['ok' => true]);
}

/* ---------- 管理：令牌鉴权 ---------- */
if (!fb_check_token((string)($in['user'] ?? ''), (string)($in['token'] ?? ''))) {
  st_fail('未登录或登录已过期，请重新登录', 401);
}

/**
 * 并表视图：遥测设备 + 反馈系统信息伪设备。
 * 每行 = [anonId, firstSeen, lastSeen, reports, fields...]，按 lastSeen 倒序。
 */
function st_all_rows() {
  $db = st_db_load(ST_CLIENT_DB, 'client');
  $fb = fb_db_load();
  $rows = [];
  foreach ($db['clients'] as $id => $c) {
    if (!is_array($c)) continue;
    $rows[] = array_merge(['anonId' => (string)$id, 'firstSeen' => (int)($c['firstSeen'] ?? 0),
      'lastSeen' => (int)($c['lastSeen'] ?? 0), 'reports' => (int)($c['reports'] ?? 1), 'src' => 'telemetry'],
      is_array($c['fields'] ?? null) ? $c['fields'] : []);
  }
  foreach ($fb['items'] as $it) {
    if (!is_array($it) || !is_array($it['systemInfo'] ?? null) || !count($it['systemInfo'])) continue;
    $ts = (int)($it['ts'] ?? 0);
    $f = $it['systemInfo'];
    if (($it['appVersion'] ?? '') !== '') $f['软件版本'] = (string)$it['appVersion'];
    $rows[] = array_merge(['anonId' => 'fb:' . (string)($it['id'] ?? ''), 'firstSeen' => $ts,
      'lastSeen' => $ts, 'reports' => 1, 'src' => 'feedback'], $f);
  }
  usort($rows, function ($a, $b) { return (int)$b['lastSeen'] <=> (int)$a['lastSeen']; });
  return $rows;
}

/** 看板聚合：卡片 + 分布 TOP10 + 30 天活跃趋势（按 lastSeen/activity 口径） */
function st_summary_data() {
  $rows = st_all_rows();
  $db = st_db_load(ST_CLIENT_DB, 'client');
  $now = st_now_ms();
  $weekAgo = $now - 7 * 24 * 3600 * 1000;
  $dayAgo = $now - 24 * 3600 * 1000;
  $by = [];
  foreach (['软件版本' => 'byVersion', '操作系统' => 'byOs', 'CPU' => 'byCpu', '内存' => 'byMem',
    '磁盘容量' => 'byDisk', '显卡' => 'byGpu', '系统语言代码' => 'byLang', '地域' => 'byRegion'] as $f => $k) $by[$k] = [];
  foreach ($rows as $r) {
    foreach ($by as $f => $k) {
      $v = trim((string)($r[$f] ?? ''));
      if ($f === '操作系统') $v = preg_replace('/\(Build [^)]*\)/', '', $v);
      if ($f === '地域' && $v === '') $v = '未知';
      if ($v !== '') $by[$k][$v] = ($by[$k][$v] ?? 0) + 1;
    }
  }
  foreach ($by as $k => $bucket) { arsort($bucket); $by[$k] = $bucket; }
  // 30 天趋势：遥测按 activity 每日去重设备数；反馈按每日提交条数
  $daily = [];
  foreach (($db['activity'] ?? []) as $anonId => $dates) {
    if (!is_array($dates)) continue;
    foreach ($dates as $k => $v) if (strtotime((string)$k) !== false) $daily[(string)$k] = ($daily[(string)$k] ?? 0) + 1;
  }
  foreach ($rows as $r) {
    if (($r['src'] ?? '') !== 'feedback') continue;
    $d = date('Y-m-d', (int)($r['lastSeen'] / 1000));
    $daily[$d] = ($daily[$d] ?? 0) + 1;
  }
  $trend = [];
  for ($i = 29; $i >= 0; $i--) {
    $d = date('Y-m-d', (int)(($now - $i * 24 * 3600 * 1000) / 1000));
    $trend[] = ['date' => $d, 'count' => (int)($daily[$d] ?? 0)];
  }
  $top = function ($bucket, $n = 10) { return array_slice($bucket, 0, $n, true); };
  return [
    'total' => count($rows),
    'weekActive' => count(array_filter($rows, function ($r) use ($weekAgo) { return (int)$r['lastSeen'] > $weekAgo; })),
    'dayActive' => count(array_filter($rows, function ($r) use ($dayAgo) { return (int)$r['lastSeen'] > $dayAgo; })),
    'byVersion' => $top($by['byVersion']), 'byOs' => $top($by['byOs']), 'byCpu' => $top($by['byCpu']),
    'byMem' => $top($by['byMem']), 'byDisk' => $top($by['byDisk']), 'byGpu' => $top($by['byGpu']),
    'byLang' => $top($by['byLang']), 'byRegion' => $top($by['byRegion']),
    'trend' => $trend,
    'full' => $by, // 全量桶（卡片明细弹窗用）
  ];
}

switch ($action) {
  case 'summary':
    st_json_out(['ok' => true, 'summary' => st_summary_data()]);

  case 'list': {
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = min(200, max(1, (int)($in['pageSize'] ?? 20)));
    $rows = st_all_rows();
    $total = count($rows);
    st_json_out(['ok' => true, 'rows' => array_slice($rows, ($page - 1) * $pageSize, $pageSize), 'total' => $total]);
  }

  case 'log': {
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = min(200, max(1, (int)($in['pageSize'] ?? 20)));
    $db = st_db_load(ST_CLIENT_DB, 'client');
    $rows = array_reverse(array_values($db['log'] ?? []));
    // 联查设备档案补充展示字段
    foreach ($rows as $i => $r) {
      $id = (string)($r['id'] ?? '');
      $c = $db['clients'][$id] ?? null;
      $f = is_array($c['fields'] ?? null) ? $c['fields'] : [];
      $rows[$i] = array_merge($r, [
        'os' => (string)($f['操作系统'] ?? ''), 'arch' => (string)($f['系统架构'] ?? ''),
        'cpu' => (string)($f['CPU'] ?? ''), 'region' => (string)($f['地域'] ?? ''),
        'clientType' => (string)($f['客户端类型'] ?? ''),
      ]);
    }
    // 反馈系统信息提交流水（并入展示，标注来源）
    $fb = fb_db_load();
    foreach ($fb['items'] as $it) {
      if (!is_array($it) || !is_array($it['systemInfo'] ?? null) || !count($it['systemInfo'])) continue;
      $f = $it['systemInfo'];
      $rows[] = ['t' => (int)($it['ts'] ?? 0), 'id' => 'fb:' . (string)($it['id'] ?? ''),
        'v' => (string)($it['appVersion'] ?? ''), 'src' => 'feedback',
        'os' => (string)($f['操作系统'] ?? ''), 'arch' => (string)($f['系统架构'] ?? ''),
        'cpu' => (string)($f['CPU'] ?? ''), 'region' => (string)($f['地域'] ?? ''),
        'clientType' => (string)($f['客户端类型'] ?? '')];
    }
    usort($rows, function ($a, $b) { return (int)($b['t'] ?? 0) <=> (int)($a['t'] ?? 0); });
    $total = count($rows);
    st_json_out(['ok' => true, 'rows' => array_slice($rows, ($page - 1) * $pageSize, $pageSize), 'total' => $total]);
  }

  case 'bucket_list': {
    $fieldMap = ['byVersion' => '软件版本', 'byOs' => '操作系统', 'byCpu' => 'CPU', 'byMem' => '内存',
      'byDisk' => '磁盘容量', 'byGpu' => '显卡', 'byLang' => '系统语言代码', 'byRegion' => '地域'];
    $bucket = (string)($in['bucket'] ?? '');
    if (!isset($fieldMap[$bucket])) st_fail('维度非法');
    $value = trim((string)($in['value'] ?? ''));
    if ($value === '') st_fail('取值非法');
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = min(200, max(1, (int)($in['pageSize'] ?? 10)));
    $rows = [];
    foreach (st_all_rows() as $r) {
      $v = trim((string)($r[$fieldMap[$bucket]] ?? ''));
      if ($bucket === 'byOs') $v = preg_replace('/\(Build [^)]*\)/', '', $v);
      if ($bucket === 'byRegion' && $v === '') $v = '未知';
      if ($v !== $value) continue;
      $rows[] = $r;
    }
    $total = count($rows);
    st_json_out(['ok' => true, 'rows' => array_slice($rows, ($page - 1) * $pageSize, $pageSize), 'total' => $total]);
  }

  case 'purge':
    st_db_save(ST_CLIENT_DB, st_db_default('client'));
    st_json_out(['ok' => true]);

  default:
    st_fail('未知动作');
}
