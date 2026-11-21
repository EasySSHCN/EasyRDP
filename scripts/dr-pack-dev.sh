#!/usr/bin/env bash
# dr-pack-dev.sh —— 重建开发角色源码包与 zip
set -e
SRC=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace
DR=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/dr-build/dev-machine
cd "$SRC"
rm -f EasyRDP/dr-build/dev-machine/EasyRDP.tar.gz 2>/dev/null || true
echo "== 生成 EasyRDP.tar.gz =="
tar -czf EasyRDP/dr-build/dev-machine/EasyRDP.tar.gz \
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
ls -lh EasyRDP/dr-build/dev-machine/EasyRDP.tar.gz

echo "== 打包开发角色 zip =="
cd "$DR"
rm -f ../dev-machine.zip
zip -r -q ../dev-machine.zip EasyRDP.tar.gz tools install-windows.ps1 install-linux.sh RESTORE-DEV.md
ls -lh ../dev-machine.zip
echo "✅ 开发角色 zip 完成"
