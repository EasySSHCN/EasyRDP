#!/usr/bin/env bash
# nginx-agent-051.sh —— gateway-proxy.conf 追加 ^~ /agent/（目标机反连；/admin/* 不暴露）
set -u
KEY=/tmp/id_ed
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY" && chmod 600 "$KEY"
trap 'rm -f "$KEY"' EXIT
ssh -q -i "$KEY" -p 22 root@easyrdp.cn 'set -e
  CONF=/www/server/panel/vhost/nginx/extension/easyrdp.cn/gateway-proxy.conf
  if grep -q "location \^~ /agent/" "$CONF"; then echo ALREADY_PRESENT; exit 0; fi
  cp "$CONF" "$CONF.bak-agent-051"
  cat >> "$CONF" <<EOF
# 目标机 Agent 反连（R-20261122-006，v0.5.1 起放行；/admin/* 仍不暴露公网）
location ^~ /agent/ {
    proxy_pass http://127.0.0.1:8443;
    proxy_http_version 1.1;
    proxy_set_header Upgrade \$http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header Host \$host;
    proxy_set_header X-Real-IP \$remote_addr;
    proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto https;
    proxy_read_timeout 3600s;
    proxy_send_timeout 3600s;
    proxy_buffering off;
}
EOF
  nginx -t
  nginx -s reload
  echo INSERTED
'
echo "==> 公网复验"
curl -s -o /dev/null -w "GET /agent/connect(期望401,非404): %{http_code}\n" https://easyrdp.cn/agent/connect
curl -s -w "\nPOST /agent/enroll(期望JSON非404): %{http_code}\n" -X POST https://easyrdp.cn/agent/enroll -H "Content-Type: application/json" -d "{}" | head -c 200
curl -s -o /dev/null -w "POST /admin/resource(期望404不暴露): %{http_code}\n" -X POST https://easyrdp.cn/admin/resource
curl -s -o /dev/null -w "/api/resources(期望401回归): %{http_code}\n" https://easyrdp.cn/api/resources
curl -s -o /dev/null -w "/tunnel/ws(期望401回归): %{http_code}\n" https://easyrdp.cn/tunnel/ws
