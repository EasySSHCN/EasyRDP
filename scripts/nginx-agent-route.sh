#!/usr/bin/env bash
# nginx-agent-route.sh —— 为目标机 Agent 反连放行 ^~ /agent/（/admin/* 保持不暴露）
set -u
KEY_PATH=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY_PATH" && chmod 600 "$KEY_PATH"
trap 'rm -f "$KEY_PATH"' EXIT
ssh -i "$KEY_PATH" -p 22 root@easyrdp.cn 'set -e
  CONF=$(grep -rl "easyrdp" /www/server/panel/vhost/nginx/ /etc/nginx/ 2>/dev/null | grep -v ".conf.bak" | head -1)
  echo "conf=$CONF"
  if ! grep -q "location \^~ /agent/" "$CONF"; then
    cp "$CONF" "$CONF.bak-agent-051"
    # 在 ^~ /tunnel/ 块后插入 agent 块（同 proxy 目标）
    TUN=$(grep -n "location \^~ /tunnel/" "$CONF" | head -1 | cut -d: -f1)
    LINE=$(awk "NR>=$TUN && /^    }/{print NR; exit}" "$CONF")
    sed -i "${LINE}a\\    location ^~ /agent/ {\\n        proxy_pass http://127.0.0.1:8443;\\n        proxy_http_version 1.1;\\n        proxy_set_header Upgrade \$http_upgrade;\\n        proxy_set_header Connection \"upgrade\";\\n        proxy_set_header Host \$host;\\n        proxy_read_timeout 3600s;\\n        proxy_send_timeout 3600s;\\n    }" "$CONF"
    nginx -t
    systemctl reload nginx || nginx -s reload
    echo "INSERTED"
  else
    echo "ALREADY_PRESENT"
  fi
'
echo "==> 公网复验"
curl -s -o /dev/null -w "GET /agent/connect(期望401,非404): %{http_code}\n" https://easyrdp.cn/agent/connect
curl -s -o /dev/null -w "POST /agent/enroll(期望400/422,非404): %{http_code}\n" -X POST https://easyrdp.cn/agent/enroll -H "Content-Type: application/json" -d "{}"
curl -s -o /dev/null -w "POST /admin/resource(期望404,不暴露): %{http_code}\n" -X POST https://easyrdp.cn/admin/resource -H "Content-Type: application/json" -d "{}"
curl -s -o /dev/null -w "/admin/audit(期望404): %{http_code}\n" https://easyrdp.cn/admin/audit
curl -s -o /dev/null -w "/tunnel/ 路由不受影响(期望401/400): %{http_code}\n" "https://easyrdp.cn/tunnel/ws"
