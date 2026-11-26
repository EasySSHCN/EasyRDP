#!/usr/bin/env bash
# gw-deploy-051.sh —— 服务器端 Linux 网关升级 v0.5.1（R-20261122-006 上线）
set -u
KEY_PATH=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY_PATH" && chmod 600 "$KEY_PATH"
trap 'rm -f "$KEY_PATH"' EXIT
ssh -i "$KEY_PATH" -p 22 root@easyrdp.cn 'set -e
  rm -rf /tmp/gwbuild
  git clone --depth 1 --branch v0.5.1 https://gitee.com/easyssh/EasyRDP.git /tmp/gwbuild 2>&1 | tail -1
  cd /tmp/gwbuild/easyrdp-gateway
  export CARGO_TARGET_DIR=/tmp/gwtgt
  cargo build --release 2>&1 | tail -2
  systemctl stop easyrdp-gateway || true
  install -m 755 /tmp/gwtgt/release/easyrdp-gateway /opt/easyrdp-gateway/easyrdp-gateway
  file /opt/easyrdp-gateway/easyrdp-gateway | head -1
  grep -q "\[agent\]" /opt/easyrdp-gateway/config.toml || printf "\n[agent]\nenabled = true\n" >> /opt/easyrdp-gateway/config.toml
  grep -A1 "\[agent\]" /opt/easyrdp-gateway/config.toml | head -3
  systemctl start easyrdp-gateway
  sleep 1
  systemctl is-active easyrdp-gateway
  ss -ltnp | grep 8443 | head -1
  echo "-- 新端点探针（无凭据，期望 401/403）--"
  curl -s -o /dev/null -w "GET /agent/connect(期望401): %{http_code}\n" http://127.0.0.1:8443/agent/connect
  curl -s -o /dev/null -w "POST /admin/resource(期望403): %{http_code}\n" -X POST http://127.0.0.1:8443/admin/resource -H "Content-Type: application/json" -d "{}"
  curl -s -o /dev/null -w "POST /agent/enroll(期望400/401): %{http_code}\n" -X POST http://127.0.0.1:8443/agent/enroll -H "Content-Type: application/json" -d "{}"
  rm -rf /tmp/gwbuild /tmp/gwtgt
'
echo "==> 公网复验"
curl -s -o /dev/null -w "GET /agent/connect(期望401): %{http_code}\n" https://easyrdp.cn/agent/connect
curl -s -o /dev/null -w "POST /admin/resource(期望403): %{http_code}\n" -X POST https://easyrdp.cn/admin/resource -H "Content-Type: application/json" -d "{}"
curl -s -o /dev/null -w "/api/resources(期望401): %{http_code}\n" https://easyrdp.cn/api/resources
curl -s -o /dev/null -w "gw-admin.html(期望200): %{http_code}\n" https://easyrdp.cn/gw-admin.html
