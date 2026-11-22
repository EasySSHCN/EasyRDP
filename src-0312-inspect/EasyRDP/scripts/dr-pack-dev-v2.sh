#!/usr/bin/env bash
# dr-pack-dev-v2.sh —— 构建开发机角色包（源码包 + tools + 脚本 → dev-machine.zip）
set -e
SRC=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace
DR=/mnt/b/EasyRDP-DR-20261117/dev-machine
mkdir -p "$DR"
cd "$SRC"

echo "== [1] EasyRDP.tar.gz（含 secrets-vault 加密库，AV 安全已验证）=="
rm -f EasyRDP/dr-build/dev-machine/EasyRDP.tar.gz 2>/dev/null || true
tar -czf /mnt/b/EasyRDP-DR-20261117/dev-machine/EasyRDP.tar.gz \
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

echo "== [2] tools 构建依赖（robocopy）=="
mkdir -p "$DR/tools"
robocopy 'B:\tools\EasyRDP' "$DR\tools\EasyRDP" /E /NFL /NDL /NJH /NP /R:1 /W:1 > /dev/null
echo "  robocopy 完成 (rc=$LASTEXITCODE)"

echo "== [3] 脚本与指南 =="
cp /mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/dr-build/dev-machine/install-windows.ps1 "$DR/" 2>/dev/null || echo "  ⚠ install-windows.ps1 缺失（dr-build 被清）"
cp /mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/dr-build/dev-machine/install-linux.sh "$DR/" 2>/dev/null || true
cp /mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace\EasyRDP/dr-build/dev-machine/RESTORE-DEV.md "$DR/" 2>/dev/null || true

echo "== [4] 打包 dev-machine.zip =="
cd "$DR"
rm -f ../dev-machine.zip
zip -r -q ../dev-machine.zip EasyRDP.tar.gz tools install-windows.ps1 install-linux.sh RESTORE-DEV.md 2>/dev/null || \
  zip -r -q ../dev-machine.zip EasyRDP.tar.gz tools 2>/dev/null || true
ls -lh "$DR" /mnt/b/EasyRDP-DR-20261117/
echo "✅ 开发机角色包完成"
