#!/usr/bin/env bash
set -e
KEY=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"; chmod 600 "$KEY"
ssh -i "$KEY" -p 22 root@198.2.231.95 '
  echo "== client-db 最新设备字段（验证地域写入）=="
  php -r "
    \$db = json_decode(file_get_contents(\"/www/wwwroot/easyrdp-data/stats/client-db.json\"), true);
    \$c = end(\$db[\"clients\"]);
    echo \"anonId=\" . key(\$db[\"clients\"]) . \"\n\";
    echo \"地域=\" . (\$c[\"fields\"][\"地域\"] ?? \"(无)\") . \"\n\";
  "
  echo "== 提交流水最新条 =="
  php -r "
    \$db = json_decode(file_get_contents(\"/www/wwwroot/easyrdp-data/stats/client-db.json\"), true);
    \$l = end(\$db[\"log\"]);
    echo json_encode(\$l, JSON_UNESCAPED_UNICODE) . \"\n\";
  "
'
rm -f "$EK"
