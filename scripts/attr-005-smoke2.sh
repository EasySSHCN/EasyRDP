#!/usr/bin/env bash
set -u
KEY_PATH=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY_PATH" && chmod 600 "$KEY_PATH"
trap 'rm -f "$KEY_PATH"' EXIT
TID="$1"
ssh -i "$KEY_PATH" -p 22 root@easyrdp.cn "bash -s" -- "$TID" <<'EOS'
TID="$1"
curl -s -X POST https://easyrdp.cn/api/stats/client.php -H "Content-Type: application/json" -d "{\"action\":\"report\",\"anonId\":\"e2e0420a00000000000000000000a005\",\"ticket_id\":\"$TID\",\"fields\":{\"软件版本\":\"0.4.2\"}}"
echo
php -r '$d=json_decode(file_get_contents("/www/wwwroot/easyrdp-data/stats/client-db.json"),true);$c=$d["clients"]["e2e0420a00000000000000000000a005"]??null;echo "origin=",json_encode($c["origin"]??null,JSON_UNESCAPED_UNICODE),PHP_EOL;'
php -r '$d=json_decode(file_get_contents("/www/wwwroot/easyrdp-data/tickets/tickets.json"),true);$t=$d["tickets"][$argv[1]]??null;echo "ticket=",json_encode(["src"=>$t["src"]??null,"url"=>$t["url"]??null,"attributed"=>$t["attributed"]??null],JSON_UNESCAPED_UNICODE),PHP_EOL;' "$TID"
php -r '$f="/www/wwwroot/easyrdp-data/stats/client-db.json";$d=json_decode(file_get_contents($f),true);unset($d["clients"]["e2e0420a00000000000000000000a005"]);file_put_contents($f,json_encode($d,JSON_UNESCAPED_UNICODE|JSON_UNESCAPED_SLASHES),LOCK_EX);echo "test device removed",PHP_EOL;'
EOS
