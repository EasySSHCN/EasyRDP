#!/usr/bin/env bash
set -e
EK=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"
ssh -i "$EK" -p 22 root@198.2.231.95 '
  php -r "
    \$db = json_decode(file_get_contents(\"/www/wwwroot/easyrdp-data/stats/client-db.json\"), true);
    foreach (array_reverse(\$db[\"log\"]) as \$l) {
      if ((string)(\$l[\"id\"] ?? \"\") === \"497a22fefc62410033708af9faaf1465c065b6e093d36b365e94af900cbd9479\") {
        echo \"log entry: \" . json_encode(\$l, JSON_UNESCAPED_UNICODE) . \"\n\";
        \$c = \$db[\"clients\"][\$l[\"id\"]] ?? null;
        echo \"client fields 地域: \" . (\$c[\"fields\"][\"地域\"] ?? \"(无)\") . \"\n\";
        echo \"client reports: \" . (\$c[\"reports\"] ?? \"-\") . \"\n\";
        break;
      }
    }
  "
'
rm -f "$EK"
