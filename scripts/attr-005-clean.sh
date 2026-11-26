#!/usr/bin/env bash
set -u
KEY_PATH=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY_PATH" && chmod 600 "$KEY_PATH"
ssh -i "$KEY_PATH" -p 22 root@easyrdp.cn "php -r '\$f=\"/www/wwwroot/easyrdp-data/tickets/tickets.json\";\$d=json_decode(file_get_contents(\$f),true);\$a=array_values(array_diff(\$d[\"tickets\"][\"62c8dc52d408133cccb7d985232825b0\"][\"attributed\"]??[],[\"e2e0420a00000000000000000000a005\"]));if(isset(\$d[\"tickets\"][\"62c8dc52d408133cccb7d985232825b0\"])){\$d[\"tickets\"][\"62c8dc52d408133cccb7d985232825b0\"][\"attributed\"]=\$a;file_put_contents(\$f,json_encode(\$d,JSON_UNESCAPED_UNICODE|JSON_UNESCAPED_SLASHES),LOCK_EX);}echo \"cleaned\",PHP_EOL;'"
rm -f "$KEY_PATH"
