#!/bin/bash
set -e
ROOT="/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP"
HOST_=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$ROOT/secrets-vault/site-server.enc" -pass file:"$ROOT/secrets-vault/key" | sed -n 's/^host: \([^ ]*\).*/\1/p')
KEY=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY" && chmod 600 "$KEY"
ssh -i "$KEY" "$HOST_" 'chown www:www /www/wwwroot/easyrdp.cn/downloads/EasyRDP-0.4.0-portable.exe /www/wwwroot/easyrdp.cn/downloads/EasyRDP-0.4.0-win32-x64-portable.zip && ls -l /www/wwwroot/easyrdp.cn/downloads/EasyRDP-0.4.0*'
rm -f "$KEY"
