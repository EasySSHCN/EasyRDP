#!/bin/bash
set -e
ROOT="/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP"
HOST_=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$ROOT/secrets-vault/site-server.enc" -pass file:"$ROOT/secrets-vault/key" | sed -n 's/^host: \([^ ]*\).*/\1/p')
KEY=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY" && chmod 600 "$KEY"
scp -q -i "$KEY" "$ROOT/easyrdp-site/download.html" "$HOST_:/www/wwwroot/easyrdp.cn/download.html"
ssh -i "$KEY" "$HOST_" 'chown www:www /www/wwwroot/easyrdp.cn/download.html && ls -l /www/wwwroot/easyrdp.cn/downloads/ | grep -E "0.3.13|0.4.0"'
rm -f "$KEY"
echo "-- live check --"
curl -s "https://easyrdp.cn/download.html?nocache=$RANDOM" | grep -o 'v0.3.13</td><td>[0-9-]*</td><td><a href="downloads/EasyRDP-0.3.13-portable.exe"' | head -1
curl -s -o /dev/null -w "exe HTTP %{http_code}\n" -r 0-256 "https://easyrdp.cn/downloads/EasyRDP-0.4.0-portable.exe"
