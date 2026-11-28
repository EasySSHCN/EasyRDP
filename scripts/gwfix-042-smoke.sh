#!/usr/bin/env bash
set -u
KEY_PATH=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY_PATH" && chmod 600 "$KEY_PATH"
ssh -i "$KEY_PATH" -p 22 root@easyrdp.cn 'curl -s -X POST https://easyrdp.cn/api/gateway/issue.php -H "Content-Type: application/json" -d "{\"action\":\"register\",\"email\":\"smoke-042@test.local\",\"password\":\"smoketest-042-pass\"}" | head -c 200; echo
sqlite3 /www/wwwroot/easyrdp-data/users.db "select id,email from users" 2>/dev/null || ls -l /www/wwwroot/easyrdp-data/users.db
rm -f /tmp/id_ed25519_easyssh'
curl -s -X POST https://easyrdp.cn/api/gateway/issue.php -H 'Content-Type: application/json' -d '{"action":"login","email":"smoke-042@test.local","password":"smoketest-042-pass"}' | head -c 200; echo
