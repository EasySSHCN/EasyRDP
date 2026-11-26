<?php
/**
 * backfill-attr.php —— 历史访客-设备回溯归因（一次性运维脚本）
 *
 * 对 client-db 中所有尚无 origin 的设备，基于全量历史数据做归因分析：
 *   A. 票据匹配：tickets.json 中存在 ip 段与设备地域掩码前缀一致、签发时间在
 *      设备 firstSeen 前 7 天内（且 ≤ firstSeen）的票据 → 经票据关联 visitId。
 *   B. IP 段兜底：visit-db recent 全量流水（不止 1000 条截断内的窗口）中，
 *      「同 /16 段（IPv6 /32）× firstSeen 前 24 小时内」最近一次访问 → 直接关联。
 *   A 优先；均匹配不到则跳过（不写）。
 * 只写空 origin（不覆盖已有归因）；每次写带 via:'backfill', confidence:'low',
 * backfill:true 便于后台识别为回溯归因。
 *
 * 用法：
 *   php backfill-attr.php --dry-run   仅打印分析结果，不写库
 *   php backfill-attr.php --apply     备份后落库
 */
set_error_handler(function ($n, $s, $f, $l) { throw new ErrorException($s, 0, $n, $f, $l); });

$DATA = '/www/wwwroot/easyrdp-data';
$apply = in_array('--apply', $argv ?? [], true);

function loadj($p) { return json_decode((string)file_get_contents($p), true) ?: []; }
function seg_of($ip) {
  $ip = trim((string)$ip);
  if (preg_match('/^(\d{1,3}\.\d{1,3})\.x\.x$/', $ip, $m)) return $m[1];
  if (strpos($ip, ':') !== false) { // IPv6 /32 = 前 2 组
    $p = explode(':', $ip); return strtolower($p[0] . ':' . ($p[1] ?? '0'));
  }
  $p = explode('.', $ip);
  return count($p) === 4 ? $p[0] . '.' . $p[1] : null;
}
function dev_prefix($region) { // 设备地域 = "中文名 a.b.x.x" 或 "a.b.x.x"
  if (preg_match('/(\d{1,3}\.\d{1,3}\.x\.x)/', (string)$region, $m)) return $m[1];
  if (preg_match('/([0-9a-fA-F:]+::)/', (string)$region, $m)) return $m[1];
  return null;
}

$cdbPath = "$DATA/stats/client-db.json";
$vdbPath = "$DATA/stats/visit-db.json";
$tdbPath = "$DATA/tickets/tickets.json";

$cdb = loadj($cdbPath);
$vdb = loadj($vdbPath);
$tdb = loadj($tdbPath);
$now = (int)(microtime(true) * 1000);

// 全量访客流水（最新在前），预计算段前缀
$visits = [];
foreach (array_reverse(is_array($vdb['recent'] ?? null) ? $vdb['recent'] : []) as $r) {
  if (!is_array($r)) continue;
  $ip = trim((string)($r['f'] ?? $r['ip'] ?? ''));
  $seg = seg_of($ip);
  if ($seg === null) continue;
  $visits[] = ['id' => (string)($r['id'] ?? ''), 't' => (int)($r['t'] ?? 0),
    'uv' => (string)($r['uv'] ?? ''), 'ip' => $ip, 'seg' => $seg,
    'p' => (string)($r['p'] ?? ''), 'rf' => (string)($r['rf'] ?? '')];
}

// 票据索引：段 → [ticket]
$ticketBySeg = [];
foreach (($tdb['tickets'] ?? []) as $tid => $t) {
  if (!is_array($t)) continue;
  $seg = seg_of((string)($t['ip'] ?? ''));
  if ($seg === null) continue;
  $ticketBySeg[$seg][] = ['id' => (string)$tid, 'ts' => (int)($t['ts'] ?? 0),
    'ip' => (string)($t['ip'] ?? ''), 'uv' => (string)($t['uv'] ?? ''),
    'visitId' => (string)($t['visitId'] ?? ''), 'src' => (string)($t['src'] ?? ''),
    'rf' => (string)($t['rf'] ?? $t['url'] ?? '')];
}

$plans = []; $skip = 0; $hasOrigin = 0;
foreach (($cdb['clients'] ?? []) as $id => $c) {
  if (!is_array($c)) continue;
  if (isset($c['origin']) && is_array($c['origin']) && count($c['origin'])) { $hasOrigin++; continue; }
  $first = (int)($c['firstSeen'] ?? 0);
  $seg = dev_prefix($c['fields']['地域'] ?? '');
  if ($seg === null || $first <= 0) { $skip++; continue; }
  $devSeg = seg_of($seg);
  if ($devSeg === null) { $skip++; continue; }

  $plan = null;
  // A. 票据：段一致 + 签发时间 ≤ firstSeen 且 firstSeen - ts ≤ 7d，取最近
  $cand = null;
  foreach (($ticketBySeg[$devSeg] ?? []) as $t) {
    if ($t['ts'] > $first || $first - $t['ts'] > 7 * 24 * 3600 * 1000) continue;
    if ($cand === null || $t['ts'] > $cand['ts']) $cand = $t;
  }
  if ($cand !== null) {
    // 经票据补齐 visitId/uv（票据未带 visitId 时找票据时间 ±5min 内同段访问）
    $vid = $cand['visitId']; $vuv = $cand['uv']; $vip = $cand['ip']; $vts = $cand['ts'];
    if ($vid === '') {
      foreach ($visits as $v) {
        if ($v['seg'] === $devSeg && abs($v['t'] - $cand['ts']) <= 5 * 60 * 1000) {
          $vid = $v['id']; $vuv = $v['uv'] ?: $vuv; $vip = $v['ip']; $vts = $v['t']; break;
        }
      }
    }
    $plan = ['method' => 'ticket', 'ticketId' => $cand['id'],
      'src' => $cand['src'] !== '' ? $cand['src'] : 'direct',
      'visitId' => $vid, 'uv' => $vuv, 'ip' => $vip, 'visitTs' => $vts];
  }
  // B. IP 段兜底：firstSeen 前 24h 内同段最近访问（流水最新在前，超窗即停不了因为乱序窗可>firstSeen，直接全扫取最近）
  if ($plan === null) {
    $best = null;
    foreach ($visits as $v) {
      if ($v['seg'] !== $devSeg) continue;
      if ($v['t'] > $first || $first - $v['t'] > 24 * 3600 * 1000) continue;
      if ($best === null || $v['t'] > $best['t']) $best = $v;
    }
    if ($best !== null) {
      $plan = ['method' => 'ip-segment', 'ticketId' => null,
        'src' => 'ip-segment', 'visitId' => $best['id'], 'uv' => $best['uv'],
        'ip' => $best['ip'], 'visitTs' => $best['t']];
    }
  }
  if ($plan === null) { $skip++; continue; }
  $plans[$id] = ['firstSeen' => $first, 'region' => (string)($c['fields']['地域'] ?? ''), 'plan' => $plan];
}

echo "=== 回溯归因分析 " . ($apply ? '(APPLY)' : '(DRY-RUN)') . " ===\n";
echo "设备总数=" . count($cdb['clients'] ?? []) . " 已有origin=$hasOrigin 可回溯=" . count($plans) . " 无法匹配=$skip\n";
foreach ($plans as $id => $p) {
  echo sprintf("  %s | first=%s | 段=%s | %s | visitId=%s | visit=%s | ip=%s | uv=%s | ticket=%s\n",
    $id, date('m-d H:i', (int)($p['firstSeen'] / 1000)), $p['region'],
    $p['plan']['method'], $p['plan']['visitId'] ?: '-',
    date('m-d H:i', (int)($p['plan']['visitTs'] / 1000)), $p['plan']['ip'],
    $p['plan']['uv'] ?: '-', $p['plan']['ticketId'] ?: '-');
}
if (!$apply) { echo "（dry-run 未写库；确认后加 --apply 执行）\n"; exit(0); }
if (!count($plans)) { echo "无可回溯条目\n"; exit(0); }

/* 备份 */
$stamp = date('Ymd-His');
foreach ([$cdbPath, $vdbPath] as $f) {
  if (copy($f, "$f.bak-$stamp")) echo "备份: $f.bak-$stamp\n";
}

/* flock 原子写 */
$fp = fopen($cdbPath . '.lock', 'c');
flock($fp, LOCK_EX);
$fresh = loadj($cdbPath);
$done = 0;
foreach ($plans as $id => $p) {
  if (!isset($fresh['clients'][$id]) || !is_array($fresh['clients'][$id])) continue;
  if (isset($fresh['clients'][$id]['origin']) && is_array($fresh['clients'][$id]['origin']) && count($fresh['clients'][$id]['origin'])) continue;
  $pl = $p['plan'];
  $fresh['clients'][$id]['origin'] = [
    'src' => $pl['src'], 'url' => '', 'visitId' => $pl['visitId'],
    'uv' => $pl['uv'], 'ip' => $pl['ip'],
    'visitTs' => $pl['visitTs'],
    'ticketId' => $pl['ticketId'],
    'downloadAt' => $pl['ticketId'] !== null ? $pl['visitTs'] : null,
    'attributedAt' => $now, 'via' => 'backfill',
    'method' => $pl['method'], 'confidence' => 'low', 'backfill' => true,
  ];
  $done++;
}
$tmp = $cdbPath . '.tmp';
file_put_contents($tmp, json_encode($fresh, JSON_UNESCAPED_UNICODE | JSON_UNESCAPED_SLASHES), LOCK_EX);
rename($tmp, $cdbPath);
flock($fp, LOCK_UN); fclose($fp);
@chown($cdbPath, 'www'); @chgrp($cdbPath, 'www');
@chown("$cdbPath.bak-$stamp", 'www'); @chgrp("$cdbPath.bak-$stamp", 'www');
echo "完成：回写 origin 设备数=$done\n";
echo "验证：php -r '…' 或后台「已产生设备的访客记录」刷新查看\n";
