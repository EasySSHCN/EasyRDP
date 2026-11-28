#!/usr/bin/env bash
# 服务器端构建 Linux 网关 v0.4.2 并恢复服务
set -u
KEY_PATH=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY_PATH" && chmod 600 "$KEY_PATH"
trap 'rm -f "$KEY_PATH"' EXIT
ssh -i "$KEY_PATH" -p 22 root@easyrdp.cn 'set -e
  rm -rf /tmp/gwbuild
  git clone --depth 1 --branch v0.4.2 https://gitee.com/easyssh/EasyRDP.git /tmp/gwbuild 2>&1 | tail -1
  cd /tmp/gwbuild/easyrdp-gateway
  export CARGO_TARGET_DIR=/tmp/gwtgt
  cargo build --release 2>&1 | tail -3
  systemctl stop easyrdp-gateway || true
  install -m 755 /tmp/gwtgt/release/easyrdp-gateway /opt/easyrdp-gateway/easyrdp-gateway
  file /opt/easyrdp-gateway/easyrdp-gateway | head -1
  systemctl start easyrdp-gateway
  sleep 1
  systemctl is-active easyrdp-gateway
  ss -ltnp | grep 8443 | head -1
  echo "-- 本机占位登录直连（期望 401/403，allow_placeholder_login=false 生效）:"
  curl -s -o /dev/null -w "%{http_code}\n" -X POST http://127.0.0.1:8443/api/auth/login -H "Content-Type: application/json" -d "{\"oidc_code\":\"probe:admin\"}"
  rm -rf /tmp/gwbuild /tmp/gwtgt
'
echo "==> 公网复验"
curl -s -o /dev/null -w "占位登录(期望403): %{http_code}\n" -X POST https://easyrdp.cn/api/auth/login -H 'Content-Type: application/json' -d '{"oidc_code":"probe:admin"}'
curl -s -o /dev/null -w "/api/resources(期望401): %{http_code}\n" https://easyrdp.cn/api/resources
curl -s -o /dev/null -w "/admin/audit(期望404): %{http_code}\n" https://easyrdp.cn/admin/audit
curl -s -o /dev/null -w "gw-admin.html(期望200): %{http_code}\n" https://easyrdp.cn/gw-admin.html
echo "-- issue.php 空体(期望JSON 400):"; curl -s -X POST https://easyrdp.cn/api/gateway/issue.php -H 'Content-Type: application/json' -d '{}' | head -c 120; echo
