#!/bin/bash
set -e
ROOT="/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP"
HOST_=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$ROOT/secrets-vault/site-server.enc" -pass file:"$ROOT/secrets-vault/key" | sed -n 's/^host: \([^ ]*\).*/\1/p')
KEY=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY" && chmod 600 "$KEY"
ssh -i "$KEY" "$HOST_" 'grep -h "0.4.0" /www/wwwlogs/easyrdp.cn.log /www/wwwlogs/easyrdp.cn.error.log 2>/dev/null | tail -30; echo "---nginx conf downloads rules---"; grep -rn "downloads" /www/server/panel/vhost/nginx/*.conf 2>/dev/null | grep -i "easyrdp" ; grep -rn -A4 "location.*downloads" /www/server/panel/vhost/nginx/easyrdp.cn.conf 2>/dev/null | head -20'
rm -f "$KEY"
