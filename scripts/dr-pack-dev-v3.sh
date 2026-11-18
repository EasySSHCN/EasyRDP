#!/usr/bin/env bash
# dr-pack-dev-v3.sh —— 开发角色 zip（tar 输出到 B:\EasyRDP-DR-20261117\dev-machine\）
set -e
SRC=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace
DR=/mnt/b/EasyRDP-DR-20261117/dev-machine
mkdir -p "$DR"
cd "$SRC"
echo "== [1] EasyRDP.tar.gz（含 secrets-vault 加密库/最新站点源码）=="
tar -czf "$DR/EasyRDP.tar.gz" \
  --exclude 'EasyRDP/easyrdp-client/target' \
  --exclude 'EasyRDP/easyrdp-gateway/target' \
  --exclude 'EasyRDP/easyrdp-client/crates/easyrdp-rdp/target' \
  --exclude 'EasyRDP/easyrdp-client/dist' \
  --exclude 'EasyRDP/node_modules' \
  --exclude 'EasyRDP/frames-debug' \
  --exclude 'EasyRDP/dr-build' \
  --exclude 'EasyRDP/EasyRDP-src-*.tar.gz' \
  --exclude 'EasyRDP/EasyRDP-0.*-portable.zip' \
  --exclude 'EasyRDP/EasyRDP-*portable' \
  --exclude 'EasyRDP/easyrdp-site/downloads' \
  EasyRDP
ls -lh "$DR/EasyRDP.tar.gz"

echo "== [2] 打包 dev-machine.zip（tools 已就位）=="
cd "$DR"
rm -f ../dev-machine.zip
ls
zip -r -q ../dev-machine.zip EasyRDP.tar.gz tools install-windows.ps1 install-linux.sh RESTORE-DEV.md
ls -lh ../dev-machine.zip
echo "✅ 开发角色 zip 完成"
