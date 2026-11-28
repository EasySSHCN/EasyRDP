<?php
// verify-chain.php —— 验证 st_rows_merged origin 断链修复 + produced 反向标注
require '/www/wwwroot/easyrdp.cn/api/stats/common.php';
$rows = st_rows_merged();
echo "== st_rows_merged origin ==\n";
foreach ($rows as $r) {
  echo substr((string)$r['anonId'], 0, 12), ' via=',
    is_array($r['origin'] ?? null) ? ($r['origin']['via'] ?? '?') : 'NULL',
    ' conf=', is_array($r['origin'] ?? null) ? ($r['origin']['confidence'] ?? '-') : '-',
    ' visitId=', is_array($r['origin'] ?? null) ? (($r['origin']['visitId'] ?: '-') ?: '-') : '-', "\n";
}
$v = json_decode(file_get_contents('/www/wwwroot/easyrdp-data/stats/visit-db.json'), true);
$c = json_decode(file_get_contents('/www/wwwroot/easyrdp-data/stats/client-db.json'), true);
$map = [];
foreach (($c['clients'] ?? []) as $id => $x) {
  $vid = (string)(($x['origin'] ?? [])['visitId'] ?? '');
  if ($vid !== '') $map[$vid][] = substr($id, 0, 12);
}
$n = 0;
foreach (array_reverse($v['recent'] ?? []) as $r) {
  if (!isset($map[(string)($r['id'] ?? '')])) continue;
  $n++;
}
echo "== produced 视图条数 = $n ==\n";
