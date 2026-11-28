<?php
/**
 * /api/stats/visit.php —— 官网访客统计（EasyRDP 官网后台 · 📈官网访客统计标签页）
 *
 * 写入端（beacon，公开、静默）：
 *   GET  ?p=<页面文件名>&r=<来源>            → 1x1 GIF（Image 信标兼容）
 *   POST JSON { page, ref }（无 token 的打点载荷）→ { ok:true }
 *   采集：服务端取真实 IP 与 UA；UV = sha256(当日盐+IP+UA) 按日轮换；
 *         流水行 = {id,t,ip(完整IP,新记录),f(完整IP),uv(12位),ua,p,r,rf(完整来源链接)}（EasySSH 语义对齐；
 *         ADMIN-FULLIP：新记录 ip 不再掩码，旧掩码记录读端原样兼容）；
 *         同 IP 每小时 600 次频控；admin.html 排除统计。
 *
 * 读取端（管理，复用反馈管理员令牌鉴权 user+token，对齐 EasySSH api/visit/admin.php）：
 *   { action:"summary", user, token }                       → { ok, summary:{ todayPv,todayUv,todayIps,
 *       weekPv,weekUv,weekIps, totalPv,totalUv,totalIps, trend:[{date,pv,uv,ips}×30],
 *       topPages:{},topRefs:{},topUas:{}, topIps:[{ip,c,region}] } }
 *   { action:"recent",  user, token, page?, pageSize? }     → { ok, rows, total } 最近访客流水（附归属地/devices）
 *   { action:"produced",user, token, page?, pageSize? }     → { ok, rows, total } 已产生设备的访客记录（完整分页）
 *   { action:"detail",  user, token, type, scope, page?, pageSize?, filter? }
 *       type ∈ visits|uv|ip|pages|refs|uas；scope ∈ today|week|total → { ok, rows, total, kind }
 *   { action:"purge",   user, token }                       → { ok } 清空访客统计库
 */
require __DIR__ . '/common.php';

/* ---------- 写入端：beacon ---------- */
$method = $_SERVER['REQUEST_METHOD'] ?? '';
$raw = ($method === 'POST') ? (string)file_get_contents('php://input') : '';
$in = json_decode($raw, true);
if (!is_array($in)) $in = [];
$action = (string)($in['action'] ?? '');

$isAdminAction = in_array($action, ['summary', 'recent', 'produced', 'detail', 'purge'], true);

if ($isAdminAction) {
  if ($method !== 'POST') st_fail('仅支持 POST', 405);
  if (!fb_check_token((string)($in['user'] ?? ''), (string)($in['token'] ?? ''))) {
    st_fail('未登录或登录已过期，请重新登录', 401);
  }
} else {
  /* 打点路径：GET 参数或 POST JSON 载荷（page/ref；服务端以真实 IP/UA 兜底） */
  $page = st_page_of($method === 'POST' ? ($in['page'] ?? '') : ($_GET['p'] ?? ''));
  $ref = $method === 'POST' ? ($in['ref'] ?? '') : ($_GET['r'] ?? '');
  st_visit_record($page, $ref);
  if ($method === 'POST') st_json_out(['ok' => true]);
  header('Content-Type: image/gif');
  header('Cache-Control: no-store, max-age=0');
  echo base64_decode('R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7');
  exit;
}

/* ---------- 读取端：管理聚合 ---------- */
$db = st_db_load(ST_VISIT_DB, 'visit');

/**
 * 反向标注：访客流水行附 devices=[已产生设备匿名ID...]。
 * 依据 = 客户端统计设备档案 origin.visitId（EasyRDP 当前设备无 origin 归因数据，
 * 保留该挂接点：后续接入归因后 devices 自动生效，前端「→ 已产生设备」链接随数据出现）。
 */
function st_attach_devices(array &$rows) {
  $sdb = st_db_load(ST_CLIENT_DB, 'client');
  $map = [];
  foreach ($sdb['clients'] as $id => $c) {
    $vid = (string)(is_array($c['origin'] ?? null) ? ($c['origin']['visitId'] ?? '') : '');
    if ($vid === '') continue;
    if (!isset($map[$vid])) $map[$vid] = [];
    if (count($map[$vid]) < 10) $map[$vid][] = (string)$id;
  }
  foreach ($rows as &$r) {
    if (!is_array($r)) continue;
    $r['devices'] = isset($map[(string)($r['id'] ?? '')]) ? $map[$r['id']] : [];
  }
  unset($r);
}

/** 行归属地补齐（st_region 当前恒为 ''，展示端兼容） */
function st_rows_region(array &$rows) {
  foreach ($rows as &$r) {
    if (!is_array($r)) continue;
    $r['region'] = st_region((string)($r['f'] ?? $r['ip'] ?? ''));
  }
  unset($r);
}

/** ADMIN-FULLIP：确保每行返回完整 IP 字段 f。
 *  新记录 ip=f=完整 IP；旧掩码记录（无 f）则 f 原样取 ip（掩码值，无法还原，原样显示）。 */
function st_rows_fullip(array &$rows) {
  foreach ($rows as &$r) {
    if (!is_array($r)) continue;
    if (!isset($r['f']) || (string)$r['f'] === '') $r['f'] = (string)($r['ip'] ?? '');
  }
  unset($r);
}

switch ($action) {
  case 'summary': {
    $today = date('Y-m-d');
    // 本周：自然周（周一起）至今
    $weekStart = date('Y-m-d', strtotime('monday this week'));
    $totalPv = 0; $totalUvKeys = []; $totalIps = [];
    $topPages = []; $topRefs = []; $topIps = []; $topUas = [];
    foreach ($db['days'] as $d => $day) {
      if (!is_array($day)) continue;
      $totalPv += (int)($day['pv'] ?? 0);
      if (isset($day['uvs']) && is_array($day['uvs'])) {
        foreach ($day['uvs'] as $k => $v) $totalUvKeys[$d . '|' . $k] = true; // UV 按日隔离，跨日去重按日拼接
      }
      foreach (($day['pages'] ?? []) as $k => $v) $topPages[$k] = ($topPages[$k] ?? 0) + $v;
      foreach (($day['refs'] ?? []) as $k => $v) $topRefs[$k] = ($topRefs[$k] ?? 0) + $v;
      // IP 分布基于完整 IP 明细聚合（与「累计 IP 数」同口径；EasySSH 同语义）
      foreach (($day['ipsFull'] ?? []) as $k => $v) {
        $totalIps[$k] = true;
        $topIps[$k] = ($topIps[$k] ?? 0) + (int)(is_array($v) ? ($v['c'] ?? 1) : 1);
      }
      // 兼容旧库：无 ipsFull 时回退掩码桶
      if (empty($day['ipsFull']) && !empty($day['ips'])) {
        foreach (($day['ips'] ?? []) as $k => $v) {
          $totalIps[$k] = true;
          $topIps[$k] = ($topIps[$k] ?? 0) + (int)$v;
        }
      }
    }
    arsort($topPages); arsort($topRefs); arsort($topIps);
    // 浏览器家族分布：统一从最近访客流水聚合（与「最近访客」列表所见一致）
    foreach ($db['recent'] as $r) {
      $u = (string)(is_array($r) ? ($r['ua'] ?? '') : '');
      if ($u !== '') $topUas[$u] = ($topUas[$u] ?? 0) + 1;
    }
    arsort($topUas);
    // TOP10 IP 转明细数组并附归属地
    $topIpsRows = [];
    $i = 0;
    foreach ($topIps as $k => $v) {
      if ($i++ >= 10) break;
      $topIpsRows[] = ['ip' => $k, 'c' => $v, 'region' => st_region($k)];
    }
    // 近 30 天趋势 + 本周聚合
    $nowMs = st_now_ms();
    $trend = [];
    $weekPv = 0; $weekUv = 0; $weekIps = [];
    for ($i = 29; $i >= 0; $i--) {
      $d = date('Y-m-d', (int)(($nowMs - $i * 24 * 3600 * 1000) / 1000));
      $day = is_array($db['days'][$d] ?? null) ? $db['days'][$d] : [];
      $pv = (int)($day['pv'] ?? 0); $uv = (int)($day['uv'] ?? 0);
      $ipsDay = isset($day['ipsFull']) && is_array($day['ipsFull']) && count($day['ipsFull'])
        ? count($day['ipsFull']) : count(is_array($day['ips'] ?? null) ? $day['ips'] : []);
      $trend[] = ['date' => $d, 'pv' => $pv, 'uv' => $uv, 'ips' => $ipsDay];
      if ($d >= $weekStart) {
        $weekPv += $pv; $weekUv += $uv;
        $ipList = isset($day['ipsFull']) && is_array($day['ipsFull']) && count($day['ipsFull']) ? $day['ipsFull'] : (is_array($day['ips'] ?? null) ? $day['ips'] : []);
        foreach ($ipList as $k => $v) $weekIps[$k] = true;
      }
    }
    $td = is_array($db['days'][$today] ?? null) ? $db['days'][$today] : [];
    $tdIps = isset($td['ipsFull']) && is_array($td['ipsFull']) && count($td['ipsFull']) ? $td['ipsFull'] : (is_array($td['ips'] ?? null) ? $td['ips'] : []);
    st_json_out(['ok' => true, 'summary' => [
      'todayPv' => (int)($td['pv'] ?? 0), 'todayUv' => (int)($td['uv'] ?? 0),
      'todayIps' => count($tdIps),
      'weekPv' => $weekPv, 'weekUv' => $weekUv, 'weekIps' => count($weekIps),
      'totalPv' => $totalPv, 'totalUv' => count($totalUvKeys), 'totalIps' => count($totalIps),
      'trend' => $trend,
      'topPages' => array_slice($topPages, 0, 10, true),
      'topRefs' => array_slice($topRefs, 0, 10, true),
      'topUas' => array_slice($topUas, 0, 10, true),
      'topIps' => $topIpsRows,
    ]]);
  }

  case 'detail': {
    // 卡片/TOP10 明细弹窗统一接口
    // type: visits(访客流水) | uv(UV哈希聚合) | ip(完整IP明细) | pages | refs | uas
    // scope: today | week | total；filter: {p,r,ip,ua} 仅 visits 支持（与 TOP10 联动）
    $type = (string)($in['type'] ?? '');
    $scope = (string)($in['scope'] ?? 'today');
    if (!in_array($type, ['visits', 'uv', 'ip', 'pages', 'refs', 'uas'], true)) st_fail('类型非法');
    if (!in_array($scope, ['today', 'week', 'total'], true)) st_fail('范围非法');
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = min(100, max(1, (int)($in['pageSize'] ?? 10)));
    $fp = isset($in['filter']['p']) ? (string)$in['filter']['p'] : '';
    $fr = isset($in['filter']['r']) ? (string)$in['filter']['r'] : '';
    $fip = isset($in['filter']['ip']) ? (string)$in['filter']['ip'] : '';
    $fua = isset($in['filter']['ua']) ? (string)$in['filter']['ua'] : '';
    $weekStart = date('Y-m-d', strtotime('monday this week'));
    $today = date('Y-m-d');
    $inScope = function ($d) use ($scope, $today, $weekStart) {
      return $scope === 'today' ? ($d === $today) : ($scope === 'week' ? ($d >= $weekStart) : true);
    };

    if ($type === 'pages' || $type === 'refs') {
      // 标题链接：全站聚合的完整排序明细（近180天保留期内，按次数倒序，标准分页）
      $agg = [];
      foreach ($db['days'] as $d => $day) {
        if (!is_array($day) || !$inScope((string)$d)) continue;
        foreach (($type === 'pages' ? ($day['pages'] ?? []) : ($day['refs'] ?? [])) as $k => $v) {
          $agg[$k] = ($agg[$k] ?? 0) + $v;
        }
      }
      arsort($agg);
      $rows = [];
      foreach ($agg as $k => $v) $rows[] = ['name' => (string)$k, 'c' => $v];
      $total = count($rows);
      st_json_out(['ok' => true, 'rows' => array_slice($rows, ($page - 1) * $pageSize, $pageSize),
        'total' => $total, 'kind' => $type]);
    }

    if ($type === 'uas') {
      // 浏览器家族全量排序明细：统一从最近访客流水聚合（与卡片口径一致）
      $agg = [];
      foreach ($db['recent'] as $r) {
        $u = (string)(is_array($r) ? ($r['ua'] ?? '') : '');
        if ($u !== '') $agg[$u] = ($agg[$u] ?? 0) + 1;
      }
      arsort($agg);
      $rows = [];
      foreach ($agg as $k => $v) $rows[] = ['name' => (string)$k, 'c' => $v];
      $total = count($rows);
      st_json_out(['ok' => true, 'rows' => array_slice($rows, ($page - 1) * $pageSize, $pageSize),
        'total' => $total, 'kind' => 'uas']);
    }

    if ($type === 'visits') {
      // 访客流水：最近 1000 条内按时间范围与可选过滤条件筛选，最新在前
      $tMin = $scope === 'today' ? strtotime($today . ' 00:00') * 1000
        : ($scope === 'week' ? strtotime($weekStart . ' 00:00') * 1000 : 0);
      $rows = [];
      foreach ($db['recent'] as $r) {
        if (!is_array($r) || (int)($r['t'] ?? 0) < $tMin) continue;
        if ($fp !== '' && ($r['p'] ?? '') !== $fp) continue;
        if ($fr !== '' && ($r['r'] ?? '') !== $fr) continue;
        if ($fip !== '' && ($r['f'] ?? '') !== $fip && ($r['ip'] ?? '') !== $fip) continue;
        if ($fua !== '' && ($r['ua'] ?? '') !== $fua) continue;
        $rows[] = $r;
      }
      $total = count($rows);
      $rows = array_slice($rows, ($page - 1) * $pageSize, $pageSize);
      st_rows_region($rows);      // 归属地只查当页行
      st_rows_fullip($rows);      // ADMIN-FULLIP：补齐完整 IP 字段
      st_attach_devices($rows);   // 反向标注已产生设备
      st_json_out(['ok' => true, 'rows' => $rows, 'total' => $total, 'kind' => 'visits']);
    }

    if ($type === 'uv') {
      // UV 聚合：{哈希: {c:次数, last:最后日期}}（哈希按日轮换，仅统计用途）
      $agg = [];
      foreach ($db['days'] as $d => $day) {
        if (!is_array($day) || !$inScope((string)$d)) continue;
        foreach (($day['uvs'] ?? []) as $h => $v) {
          $c = is_array($v) ? (int)($v['c'] ?? 1) : 1;
          if (!isset($agg[$h])) $agg[$h] = ['uv' => $h, 'c' => 0, 'last' => (string)$d];
          $agg[$h]['c'] += $c;
          if ((string)$d > $agg[$h]['last']) $agg[$h]['last'] = (string)$d;
        }
      }
      $rows = array_values($agg);
      usort($rows, function ($a, $b) { return $b['c'] <=> $a['c']; });
      $total = count($rows);
      st_json_out(['ok' => true, 'rows' => array_slice($rows, ($page - 1) * $pageSize, $pageSize),
        'total' => $total, 'kind' => 'uv']);
    }

    // ip：完整 IP 明细聚合（IPv4/IPv6 全量，含次数与最后访问时间，附掩码形态）
    $agg = [];
    foreach ($db['days'] as $d => $day) {
      if (!is_array($day) || !$inScope((string)$d)) continue;
      foreach (($day['ipsFull'] ?? []) as $ip => $v) {
        if (!isset($agg[$ip])) $agg[$ip] = ['ip' => $ip, 'mask' => st_ip_mask($ip), 'c' => 0, 'last' => 0];
        $agg[$ip]['c'] += (int)(is_array($v) ? ($v['c'] ?? 1) : 1);
        $agg[$ip]['last'] = max($agg[$ip]['last'], (int)(is_array($v) ? ($v['last'] ?? 0) : 0));
      }
    }
    $rows = array_values($agg);
    usort($rows, function ($a, $b) { return $b['c'] <=> $a['c']; });
    $total = count($rows);
    $rows = array_slice($rows, ($page - 1) * $pageSize, $pageSize);
    foreach ($rows as &$r) $r['region'] = st_region($r['ip']);
    unset($r);
    st_json_out(['ok' => true, 'rows' => $rows, 'total' => $total, 'kind' => 'ip']);
  }

  case 'recent': {
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = min(100, max(1, (int)($in['pageSize'] ?? 20)));
    $rows = array_slice(array_values(array_filter(is_array($db['recent']) ? $db['recent'] : [], 'is_array')),
      ($page - 1) * $pageSize, $pageSize);
    st_rows_region($rows);
    st_rows_fullip($rows); // ADMIN-FULLIP：补齐完整 IP 字段
    st_attach_devices($rows); // 反向标注已产生设备
    st_json_out(['ok' => true, 'rows' => $rows, 'total' => count($db['recent'])]);
  }

  case 'produced': {
    // 「已产生设备的访客记录」：最近 1000 条流水内 devices 非空的访客记录，最新在前，完整分页
    $page = max(1, (int)($in['page'] ?? 1));
    $pageSize = min(100, max(1, (int)($in['pageSize'] ?? 10)));
    $rows = array_values(array_filter(is_array($db['recent']) ? $db['recent'] : [], 'is_array'));
    st_attach_devices($rows);
    $rows = array_values(array_filter($rows, function ($r) {
      return isset($r['devices']) && is_array($r['devices']) && count($r['devices']) > 0;
    }));
    $total = count($rows);
    $rows = array_slice($rows, ($page - 1) * $pageSize, $pageSize);
    st_rows_region($rows);
    st_rows_fullip($rows); // ADMIN-FULLIP：补齐完整 IP 字段
    st_json_out(['ok' => true, 'rows' => $rows, 'total' => $total]);
  }

  case 'purge':
    st_db_save(ST_VISIT_DB, st_db_default('visit'));
    st_json_out(['ok' => true]);

  default:
    st_fail('未知动作');
}
