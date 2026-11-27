#!/bin/bash
set -e
ROOT="/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP"
HOST_=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$ROOT/secrets-vault/site-server.enc" -pass file:"$ROOT/secrets-vault/key" | sed -n 's/^host: \([^ ]*\).*/\1/p')
KEY=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY" && chmod 600 "$KEY"
ssh -i "$KEY" "$HOST_" 'grep -n "server" /www/server/panel/vhost/nginx/easyrdp.cn.conf | head; echo ---; head -30 /www/server/panel/vhost/nginx/easyrdp.cn.conf'
rm -f "$KEY"
