#!/bin/bash
set -e
ROOT="/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP"
HOST_=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$ROOT/secrets-vault/site-server.enc" -pass file:"$ROOT/secrets-vault/key" | sed -n 's/^host: \([^ ]*\).*/\1/p')
KEY=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY" && chmod 600 "$KEY"
echo "== 当前线上 download.html 按钮 href =="
ssh -i "$KEY" "$HOST_" 'grep -o "href=\"[^\"]*EasyRDP-0.4.0-portable.exe\"" /www/wwwroot/easyrdp.cn/download.html | head -3'
echo "== vhost conf 位置 =="
ssh -i "$KEY" "$HOST_" 'ls /www/server/panel/vhost/nginx/ | grep -i easyrdp'
rm -f "$KEY"
