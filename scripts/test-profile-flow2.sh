#!/usr/bin/env bash
set -e
KEY=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"; chmod 600 "$KEY"
ssh -i "$KEY" -p 22 root@198.2.231.95 'bash -s' <<'REMOTE'
set -e
cd /www/wwwroot/easyrdp.cn/api/feedback
TOKEN=$(sudo -u www php -r 'require "common.php"; echo fb_session_create("xmmaster");')
echo "TOKEN=${TOKEN:0:16}..."
echo "== profile（模拟刷新恢复）=="
curl -sk -X POST https://127.0.0.1/api/feedback/admin.php \
  -H "Content-Type: application/json" -H "Host: easyrdp.cn" \
  -d "{\"action\":\"profile\",\"user\":\"xmmaster\",\"token\":\"$TOKEN\"}"
echo
REMOTE
rm -f "$KEY"
