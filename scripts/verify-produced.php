<?php
$c = json_decode(file_get_contents("/www/wwwroot/easyrdp-data/stats/client-db.json"), true);
$v = json_decode(file_get_contents("/www/wwwroot/easyrdp-data/stats/visit-db.json"), true);
$map = [];
foreach (($c["clients"] ?? []) as $id => $x) {
  $vid = (string)(($x["origin"] ?? [])["visitId"] ?? "");
  if ($vid !== "") $map[$vid][] = substr($id, 0, 12);
}
$n = 0;
foreach (array_reverse($v["recent"] ?? []) as $r) {
  $id = (string)($r["id"] ?? "");
  if (!isset($map[$id])) continue;
  $n++;
  echo date("m-d H:i", (int)((int)$r["t"] / 1000)), " ", ($r["p"] ?? ""), " ",
    ($r["f"] ?? $r["ip"] ?? ""), " -> devices=", implode(",", $map[$id]), "\n";
}
echo "produced 总数=$n\n";
