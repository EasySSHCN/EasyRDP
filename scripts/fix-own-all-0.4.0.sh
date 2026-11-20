#!/bin/bash
set -e
ROOT="/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP"
HOST_=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$ROOT/secrets-vault/site-server.enc" -pass file:"$ROOT/secrets-vault/key" | sed -n 's/^host: \([^ ]*\).*/\1/p')
KEY=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY" && chmod 600 "$KEY"
ssh -i "$KEY" "$HOST_" 'chown -R www:www /www/wwwroot/easyrdp.cn/downloads && ls -l /www/wwwroot/easyrdp.cn/downloads | head -8'
rm -f "$KEY"
echo "-- ticket -> get.php 全链路验证 --"
T=$(curl -s "https://easyrdp.cn/api/download/ticket.php" | sed -n 's/.*"ticket_id":"\([a-f0-9]*\)".*/\1/p')
echo "ticket: $T"
curl -s -o /dev/null -w "get.php HTTP %{http_code}\n" -r 0-256 "https://easyrdp.cn/api/download/get.php?t=$T&f=downloads/EasyRDP-0.4.0-portable.exe"
