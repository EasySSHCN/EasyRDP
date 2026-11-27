#!/usr/bin/env bash
# dr-refresh-050-resume.sh —— 续跑：远程校验 + 拉回 + 开发机角色组装
set -e
LOG=/mnt/b/EasyRDP-DR-20261122/dr-refresh-progress.log
exec >> "$LOG" 2>&1
PW='EasyRDP-DR-20261122'
DEST=/mnt/b/EasyRDP-DR-20261122
KEY=/tmp/ksv
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$KEY"; chmod 600 "$KEY"

echo "== [A2] sv 端 zip 校验（带密码）=="
ssh -i "$KEY" -p 53422 root@sv.trae.show "unzip -t -P '$PW' /root/dr-remote-server-20261122.zip >/dev/null && echo SV-ZIP-OK && ls -lh /root/dr-remote-server-20261122.zip"

echo "== [B] 拉回远程角色 zip =="
EK=/tmp/kek2
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"
scp -q -i "$EK" -P 53422 "root@sv.trae.show:/root/dr-remote-server-20261122.zip" "$DEST/remote-server.zip"
rm -f "$EK" "$KEY"

echo "== [C] 开发机角色：重建源码包 + zip =="
SRC=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace
DEV="$DEST/dev-machine"
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

echo "== [D] 组装开发机 zip（.NET 流式）=="
powershell.exe -NoProfile -Command "
  Remove-Item 'B:\EasyRDP-DR-20261122\dev-machine.zip' -Force -ErrorAction SilentlyContinue
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  [System.IO.Compression.ZipFile]::CreateFromDirectory('B:\EasyRDP-DR-20261122\dev-machine', 'B:\EasyRDP-DR-20261122\dev-machine.zip', [System.IO.Compression.CompressionLevel]::Optimal, \$false)
"

echo "== [E] 最终交付清单 =="
ls -lh "$DEST"
echo "✅ v0.5.0 灾备刷新完成"
