#!/usr/bin/env bash
# R-20261120-005 褰掑洜瀵归綈锛氶儴缃?3 涓?PHP + admin.html锛屽苟绾夸笂绔埌绔啋鐑燂紙绁ㄦ嵁鈫掑綊鍥犫啋鍥炶锛?set -u
KEY_PATH=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY_PATH" && chmod 600 "$KEY_PATH"
trap 'rm -f "$KEY_PATH"' EXIT
SITE="$(cd "$(dirname "$0")/.." && pwd)/easyrdp-site"
H=root@easyrdp.cn; P=22
W=/www/wwwroot/easyrdp.cn
scp -q -i "$KEY_PATH" -P $P "$SITE/api/stats/client.php" "$H:$W/api/stats/client.php" || exit 1
scp -q -i "$KEY_PATH" -P $P "$SITE/api/stats/common.php" "$H:$W/api/stats/common.php" || exit 1
scp -q -i "$KEY_PATH" -P $P "$SITE/api/download/ticket.php" "$H:$W/api/download/ticket.php" || exit 1
scp -q -i "$KEY_PATH" -P $P "$SITE/admin.html" "$H:$W/admin.html" || exit 1
ssh -i "$KEY_PATH" -p $P $H "chown www:www $W/api/stats/client.php $W/api/stats/common.php $W/api/download/ticket.php $W/admin.html && echo deployed"
ssh -i "$KEY_PATH" -p $P $H 'set -e
  echo "-- 鈶?绛惧彂绁ㄦ嵁锛堟潵婧?鐧惧害鎼滅储锛?"
  T=$(curl -s "https://easyrdp.cn/api/download/ticket.php?r=https%3A%2F%2Fwww.baidu.com%2Fs%3Fwd%3Deasyrdp")
  echo "$T" | head -c 200; echo
  TID=$(echo "$T" | grep -o "\"ticket_id\":\"[0-9a-f]*\"" | grep -o "[0-9a-f]\{32\}")
  echo "TID=$TID"
  [ -n "$TID" ] || { echo "鉁?鏈彇寰楃エ鎹?; exit 1; }
  echo "-- 鈶?瀹㈡埛绔鎶ユ惡甯︾エ鎹?"
  curl -s -X POST https://easyrdp.cn/api/stats/client.php -H "Content-Type: application/json" \
    -d "{\"action\":\"report\",\"anonId\":\"e2e0420a00000000000000000000a005\",\"ticket_id\":\"$TID\",\"fields\":{\"杞欢鐗堟湰\":\"0.4.2\"}}"; echo
  echo "-- 鈶?鍥炶璁惧妗ｆ origin:"
  php -r "\$d=json_decode(file_get_contents(\"/www/wwwroot/easyrdp-data/stats/client-db.json\"),true);\$c=\$d[\"clients\"][\"e2e0420a00000000000000000000a005\"]??null;echo json_encode(\$c[\"origin\"]??null,JSON_UNESCAPED_UNICODE),PHP_EOL;"
  echo "-- 鈶?绁ㄦ嵁 attributed 鏍囪:"
  php -r "\$d=json_decode(file_get_contents(\"/www/wwwroot/easyrdp-data/tickets/tickets.json\"),true);\$t=\$d[\"tickets\"][\"$TID\"]??null;echo json_encode([\"src\"=>\$t[\"src\"]??null,\"url\"=>\$t[\"url\"]??null,\"attributed\"=>\$t[\"attributed\"]??null],JSON_UNESCAPED_UNICODE),PHP_EOL;"
  echo "-- 鈶?娴嬭瘯鏁版嵁娓呯悊锛堢エ鎹?attributed + 娴嬭瘯璁惧锛?"
  php -r "\$f=\"/www/wwwroot/easyrdp-data/stats/client-db.json\";\$d=json_decode(file_get_contents(\$f),true);unset(\$d[\"clients\"][\"e2e0420a00000000000000000000a005\"]);file_put_contents(\$f,json_encode(\$d,JSON_UNESCAPED_UNICODE|JSON_UNESCAPED_SLASHES),LOCK_EX);echo \"device removed\",PHP_EOL;"
'
