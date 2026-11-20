#!/usr/bin/env bash
set -e
EK=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"
ssh -i "$EK" -p 22 root@198.2.231.95 '
  chown -R www:www /www/wwwroot/easyrdp-data/stats
  php -r "
    \$f = \"/www/wwwroot/easyrdp-data/stats/client-db.json\";
    \$db = json_decode(file_get_contents(\$f), true);
    \$id = \"497a22fefc62410033708af9faaf1465c065b6e093d36b365e94af900cbd9479\";
    // 修复设备档案字段版本号
    \$db[\"clients\"][\$id][\"fields\"][\"软件版本\"] = \"0.3.3\";
    // 修复流水条目版本号
    foreach (\$db[\"log\"] as &\$l) {
      if ((string)(\$l[\"id\"] ?? \"\") === \$id && (string)(\$l[\"v\"] ?? \"\") === \"\") \$l[\"v\"] = \"0.3.3\";
    }
    unset(\$l);
    file_put_contents(\$f, json_encode(\$db, JSON_UNESCAPED_UNICODE));
    chown(\$f, \"www\"); chgrp(\$f, \"www\");
    echo \"已修复 0.3.3\n\";
  "
'
rm -f "$EK"
