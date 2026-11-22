#!/usr/bin/env bash
set -e
EK=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"
ssh -i "$EK" -p 22 root@198.2.231.95 '
  cd /www/wwwroot/easyrdp.cn/api/feedback
  TOKEN=$(sudo -u www php -r "
    require \"common.php\";
    echo fb_session_create(\"xmmaster\");
  ")
  echo "测试令牌: ${TOKEN:0:16}..."
  echo "== 用测试令牌调 profile（模拟刷新恢复）=="
  curl -s -X POST http://127.0.0.1/api/feedback/admin.php \
    -H "Content-Type: application/json" \
    -d "{\"action\":\"profile\",\"user\":\"xmmaster\",\"token\":\"$TOKEN\"}"
  echo
'
rm -f "$EK"
