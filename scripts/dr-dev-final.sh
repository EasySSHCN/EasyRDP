#!/usr/bin/env bash
set -e
LOG=/mnt/b/EasyRDP-DR-20261122/dr-refresh-progress.log
exec >> "$LOG" 2>&1
echo "== [C] 开发机角色:重建源码包(zip 已由浏览器高速下载落位) =="
SRC=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace
DEV=/mnt/b/EasyRDP-DR-20261122/dev-machine
mkdir -p "$DEV"
cd "$SRC"
rm -f "$DEV/EasyRDP.tar.gz"
tar -czf "$DEV/EasyRDP.tar.gz" \
  --exclude 'EasyRDP/easyrdp-client/target' \
  --exclude 'EasyRDP/easyrdp-gateway/target' \
  --exclude 'EasyRDP/easyrdp-client/crates/easyrdp-rdp/target' \
  --exclude 'EasyRDP/easyrdp-client/dist' \
  --exclude 'EasyRDP/easyrdp-client/src-tauri/target' \
  --exclude 'EasyRDP/node_modules' \
  --exclude 'EasyRDP/frames-debug' \
  --exclude 'EasyRDP/dr-build' \
  --exclude 'EasyRDP/EasyRDP-src-*.tar.gz' \
  --exclude 'EasyRDP/EasyRDP-0.*-portable.zip' \
  --exclude 'EasyRDP/EasyRDP-*portable' \
  --exclude 'EasyRDP/easyrdp-site/downloads' \
  EasyRDP
ls -lh "$DEV/EasyRDP.tar.gz"

echo "== [D] 组装开发机 zip(.NET 流式) =="
powershell.exe -NoProfile -Command "
  Remove-Item 'B:\EasyRDP-DR-20261122\dev-machine.zip' -Force -ErrorAction SilentlyContinue
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  [System.IO.Compression.ZipFile]::CreateFromDirectory('B:\EasyRDP-DR-20261122\dev-machine', 'B:\EasyRDP-DR-20261122\dev-machine.zip', [System.IO.Compression.CompressionLevel]::Optimal, \$false)
"

echo "== [E] 最终交付清单 =="
ls -lh /mnt/b/EasyRDP-DR-20261122/
echo "DR-ALL-DONE"
