#!/usr/bin/env bash
# gw-deploy-052.sh —— 服务器 Linux 网关升级 v0.5.2（源码与 0.5.1 无差异，随版对齐）
set -u
KEY_PATH=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY_PATH" && chmod 600 "$KEY_PATH"
trap 'rm -f "$KEY_PATH"' EXIT
ssh -i "$KEY_PATH" -p 22 root@easyrdp.cn 'set -e
  rm -rf /tmp/gwbuild
  git clone --depth 1 --branch v0.5.2 https://gitee.com/easyssh/EasyRDP.git /tmp/gwbuild 2>&1 | tail -1
  cd /tmp/gwbuild/easyrdp-gateway
  export CARGO_TARGET_DIR=/tmp/gwtgt
  cargo build --release 2>&1 | tail -2
  systemctl stop easyrdp-gateway || true
  install -m 755 /tmp/gwtgt/release/easyrdp-gateway /opt/easyrdp-gateway/easyrdp-gateway
  systemctl start easyrdp-gateway
  sleep 1
  systemctl is-active easyrdp-gateway
  ss -ltnp | grep 8443 | head -1
  curl -s -o /dev/null -w "GET /agent/connect(400=在线): %{http_code}\n" http://127.0.0.1:8443/agent/connect
  rm -rf /tmp/gwbuild /tmp/gwtgt
'
curl -s -o /dev/null -w "公网 /api/resources(期望401): %{http_code}\n" https://easyrdp.cn/api/resources
curl -s -o /dev/null -w "公网 /agent/connect(期望400): %{http_code}\n" https://easyrdp.cn/agent/connect
