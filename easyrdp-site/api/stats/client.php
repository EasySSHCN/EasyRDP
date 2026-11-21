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
 *   R-20261119-010 IP 段兜底归因：客户端上报未携带有效票据（如无票据下载/复制安装）时，
 *     若 visit-db 最近访客流水中存在「该 IP 同段 × 24 小时内」的访问记录，则在设备档案
 *     首次写 origin{via:'ip-segment', src:'ip-segment', visitId, uv, ip, visitTs, ts,
 *     confidence:'low'}，含 UV↔设备ID↔IP↔时间四要素；visit.php produced / detail 的
 *     devices 反向标注随之生效（后台展示自动加「疑似」标注）。匹配阈值：
 *     IPv4 = 前 16 位段（a.b.x.x，st_ip_prefix 口径）；IPv6 = /32 前缀；时间窗 ≤24h。
 *     票据归因优先（有票走票据链路）；每台设备仅首次建档时兜底归因一次，不覆盖已有 origin。
 */
require __DIR__ . '/common.php';

if (!defined('ST_ATTR_WIN_MS')) {
  define('ST_ATTR_WIN_MS', 24 * 3600 * 1000); // IP 段兜底归因回溯窗：官网访问后 ≤24 小时
}

/**
 * R-20261119-010：按「同 IP 段 + 24 小时窗」回溯最近访客流水，返回最近一条匹配记录。
 * recent 为最新在前；超窗即停（早于窗内的不可能再匹配）。段口径与 st_ip_prefix 一致：
 * IPv4 = a.b（前 16 位）；IPv6 = 前 4+4 hextet（/32）。旧掩码行 "a.b.x.x" 直接取前两段。
 */
/* ---------- R-20261120-005 来源归因对齐 EasySSH：票据扫描/启发式（st_src_classify 在 common.php） ---------- */

/** 票据保留期内（90 天）均可参与扫描；窗内=high，超窗=low（EasySSH st_origin_find_ticket 同语义） */
function st_attr_ticket_age($t, $now) {
  $ts = (int)($t['ts'] ?? 0);
  if ($ts > $now + 60 * 1000) return null;                       // 未来票据（时钟偏移容忍 1 分钟）
  if ($now - $ts > 90 * 24 * 3600 * 1000) return null;           // 超保留期
  return ($now - $ts <= ST_TICKET_WINDOW_MS) ? 'high' : 'low';
}

/** 票据查找：$ticketId 非空精确查找；否则扫描同完整 IP 名下未满额票据（窗内优先最近，超窗 low） */
function st_attr_find_ticket($ip, $now, $ticketId = '') {
  $db = st_db_load(ST_TICKET_DB, 'ticket');
  $tickets = isset($db['tickets']) && is_array($db['tickets']) ? $db['tickets'] : [];
  if ($ticketId !== '') {
    $t = isset($tickets[$ticketId]) && is_array($tickets[$ticketId]) ? $tickets[$ticketId] : null;
    if ($t === null) return null;
    $conf = st_attr_ticket_age($t, $now);
    return $conf === null ? null : ['ticket' => $t, 'confidence' => $conf];
  }
  $inWin = []; $outWin = [];
  foreach ($tickets as $k => $t) {
    if (!is_array($t)) continue;
    if (isset($t['attributed']) && is_array($t['attributed']) && count($t['attributed']) >= ST_TICKET_ATTRIBUTED_MAX) continue;
    if (trim((string)($t['ip'] ?? '')) !== (string)$ip) continue; // 同完整 IP
    $conf = st_attr_ticket_age($t, $now);
    if ($conf === null) continue;
    if ($conf === 'high') $inWin[(string)$k] = $t; else $outWin[(string)$k] = $t;
  }
  $pick = function ($bucket, $conf) {
    if (!count($bucket)) return null;
    uasort($bucket, function ($a, $b) { return (int)($b['ts'] ?? 0) <=> (int)($a['ts'] ?? 0); });
    $k = (string)array_key_first($bucket);
    return ['ticketId' => $k, 'ticket' => $bucket[$k], 'confidence' => $conf];
  };
  $hit = $pick($inWin, 'high');
  if ($hit !== null) return $hit;
  return $pick($outWin, 'low');
}

/** 方案 B 启发式（EasySSH st_origin_heuristic 同语义）：同完整 IP + 设备 firstSeen 前 7 天内最近一次访问 */
function st_attr_heuristic($ip, $firstSeen, $now) {
  $win = 7 * 24 * 3600 * 1000;
  $vdb = st_db_load(ST_VISIT_DB, 'visit');
  if (!isset($vdb['recent']) || !is_array($vdb['recent'])) return null;
  $best = null;
  foreach ($vdb['recent'] as $r) {
    if (!is_array($r)) continue;
    if (trim((string)($r['f'] ?? $r['ip'] ?? '')) !== (string)$ip) continue;
    $t = (int)($r['t'] ?? 0);
    if ($t > $firstSeen || $firstSeen - $t > $win) continue;
    if ($best === null || $t > (int)($best['t'] ?? 0)) $best = $r;
  }
  if ($best === null) return null;
  $rf = (string)($best['rf'] ?? '');
  return ['src' => st_src_classify($rf), 'url' => $rf,
    'visitId' => (string)($best['id'] ?? ''), 'visitTs' => (int)($best['t'] ?? 0),
    'via' => 'heuristic', 'attributedAt' => $now];
}

/** 按 visitId 反查访客流水时间（EasySSH st_origin_visit_ts 同语义；取不到回退票据签发时间） */
function st_attr_visit_ts($visitId, $fallbackTs) {
  if ($visitId === '') return $fallbackTs;
  $vdb = st_db_load(ST_VISIT_DB, 'visit');
  if (!isset($vdb['recent']) || !is_array($vdb['recent'])) return $fallbackTs;
  foreach ($vdb['recent'] as $r) {
    if (is_array($r) && (string)($r['id'] ?? '') === (string)$visitId) return (int)($r['t'] ?? $fallbackTs);
  }
  return $fallbackTs;
}

/** 票据 → origin（EasySSH 字段口径：src/url/visitId/uv/ip/visitTs/downloadAt/attributedAt/via/confidence + ticketId） */
function st_attr_ticket_origin($ticketId, $t, $now, $confidence) {
  $rf = (string)($t['rf'] ?? $t['url'] ?? '');
  $vid = (string)($t['visitId'] ?? '');
  $o = [
    'src' => (string)($t['src'] ?? '') !== '' ? (string)$t['src'] : st_src_classify($rf),
    'url' => $rf,
    'visitId' => $vid,
    'uv' => (string)($t['uv'] ?? ''),
    'ip' => (string)($t['ip'] ?? ''),
    'visitTs' => st_attr_visit_ts($vid, (int)($t['ts'] ?? 0)),
    'ticketId' => $ticketId,
    'downloadAt' => (int)($t['ts'] ?? 0),
    'attributedAt' => $now,
    'via' => 'ticket',
    'confidence' => $confidence,
  ];
  return $o;
}

function st_attr_visit_row($ip) {
  $pp = st_ip_prefix($ip);
  if ($pp === null) return null;              // 内网/非法 IP 不参与归因
  $mySeg = (string)$pp[1];
  $cut = st_now_ms() - ST_ATTR_WIN_MS;
  $vdb = st_db_load(ST_VISIT_DB, 'visit');
  if (!isset($vdb['recent']) || !is_array($vdb['recent'])) return null;
  foreach ($vdb['recent'] as $r) {
    if (!is_array($r)) continue;
    $rt = (int)($r['t'] ?? 0);
    if ($rt < $cut) break;                    // 最新在前，超 24h 窗即停
    $rip = trim((string)($r['f'] ?? $r['ip'] ?? ''));
    if ($rip === '') continue;
    if (preg_match('/^(\d{1,3}\.\d{1,3})\.x\.x$/', $rip, $m)) { $rSeg = $m[1]; }        // 旧掩码行
    else { $rp = st_ip_prefix($rip); if ($rp === null) continue; $rSeg = (string)$rp[1]; } // 完整 IP 行
    if ($rSeg === $mySeg) return $r;
  }
  return null;
}

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
  $fields = st_sanitize(is_array($in['fields'] ?? null) ? $in['fields'] : []);
  $now = st_now_ms();
  $d = date('Y-m-d');
  // GEO-20261118：地域 = 中文归属地名（查询链+前缀缓存）+ 掩码前缀，对齐 EasySSH st_apply_region 语义
  $pp = st_ip_prefix($ip);
  if ($pp !== null) {
    $geoName = st_region($ip);
    $fields['地域'] = trim(($geoName !== '' ? $geoName . ' ' : '') . st_ip_mask_label($pp[0], $pp[1]));
  }
  // R-20261120-005 归因对齐 EasySSH：①票据精确（窗内 high/超窗 low）→ ②新设备同 IP 票据扫描
  // → ③方案 B 启发式（疑似）→ ④IP 段兜底（EasyRDP 特有，疑似）。首次建档即定，不覆盖。
  $segRow = st_attr_visit_row($ip); // flock 外只读一次（低频上报可接受）
  $usedTicketId = null;
  st_locked_update(ST_CLIENT_DB, 'client', function ($db) use ($anonId, $fields, $now, $d, $ticketId, $ip, $segRow, &$dupOut, &$usedTicketId) {
    if (!isset($db['clients'][$anonId]) || !is_array($db['clients'][$anonId])) {
      $db['clients'][$anonId] = ['firstSeen' => $now, 'lastSeen' => $now, 'reports' => 0, 'fields' => [], 'origin' => null];
    }
    $c = &$db['clients'][$anonId];
    $c['lastSeen'] = $now;
    $dup = isset($db['activity'][$anonId]) && is_array($db['activity'][$anonId])
      && isset($db['activity'][$anonId][$d]);
    $dupOut = $dup;
    $isNew = !isset($db['activity'][$anonId]) || !is_array($db['activity'][$anonId]) || !count($db['activity'][$anonId]);
    if ((!isset($c['origin']) || !is_array($c['origin']))) {
      $origin = null;
      // ① 客户端携带票据：精确匹配（窗内 high；超窗保留期内 low，展示「超窗，疑似」）
      if ($ticketId !== '') {
        $hit = st_attr_find_ticket($ip, $now, $ticketId);
        if ($hit !== null) {
          $origin = st_attr_ticket_origin($ticketId, $hit['ticket'], $now, $hit['confidence']);
          $usedTicketId = $ticketId;
        }
      }
      // ② 新设备无票：同完整 IP 票据扫描 → ③ 启发式 → ④ IP 段兜底
      if ($origin === null && $isNew) {
        $scan = st_attr_find_ticket($ip, $now);
        if ($scan !== null) {
          $origin = st_attr_ticket_origin($scan['ticketId'], $scan['ticket'], $now, $scan['confidence']);
          $usedTicketId = $scan['ticketId'];
        } else {
          $origin = st_attr_heuristic($ip, (int)($c['firstSeen'] ?? $now), $now);
        }
        if ($origin === null && $segRow !== null) {
          $origin = ['via' => 'ip-segment', 'src' => 'ip-segment',
            'visitId' => (string)($segRow['id'] ?? ''),
            'uv' => (string)($segRow['uv'] ?? ''),
            'ip' => (string)($segRow['f'] ?? $segRow['ip'] ?? ''),
            'visitTs' => (int)($segRow['t'] ?? 0),
            'attributedAt' => $now, 'confidence' => 'low'];
        }
      }
      if ($origin !== null) $c['origin'] = $origin;
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
  // 票据归因标记（命中即标记，含超窗 low；flock 原子写，单票设备数上限防灌水）
  if ($usedTicketId !== null && !$dupOut) {
    st_locked_update(ST_TICKET_DB, 'ticket', function ($db) use ($anonId, $usedTicketId, $now) {
      if (!isset($db['tickets'][$usedTicketId]) || !is_array($db['tickets'][$usedTicketId])) return $db;
      $t = &$db['tickets'][$usedTicketId];
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
      'lastSeen' => (int)($c['lastSeen'] ?? 0), 'reports' => (int)($c['reports'] ?? 1), 'src' => 'telemetry',
      'origin' => isset($c['origin']) && is_array($c['origin']) ? $c['origin'] : null],
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
  // R-20261120-005 来源分布：按设备档案 origin.src 聚合（无 origin 计入 unknown，对齐 EasySSH bySrc）
  $bySrc = [];
  foreach ($rows as $r) {
    if (($r['src'] ?? '') !== 'telemetry') continue;
    $s = isset($r['origin']['src']) && is_array($r['origin']) ? (string)$r['origin']['src'] : 'unknown';
    if ($s === '') $s = 'unknown';
    $bySrc[$s] = ($bySrc[$s] ?? 0) + 1;
  }
  arsort($bySrc);
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
    'bySrc' => array_slice($bySrc, 0, 10, true),
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
