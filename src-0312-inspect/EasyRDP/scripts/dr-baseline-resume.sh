#!/usr/bin/env bash
set -e
LOG=/mnt/b/EasyRDP-DR-20261117/dr-baseline-progress.log
exec >> "$LOG" 2>&1
echo "[$(date '+%F %T')] 续跑：校验+拉取+开发机打包"
KEY=/tmp/ksv
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$KEY"; chmod 600 "$KEY"
PW='EasyRDP-DR-20261117'
DEST=/mnt/b/EasyRDP-DR-20261117
SVSSH="ssh -i $KEY -p 53422 -o StrictHostKeyChecking=accept-new -o ServerAliveInterval=30 -o ServerAliveCountMax=20 root@sv.trae.show"

echo "== [B1] sv 端 zip 完整性校验（zip -T）=="
$SVSSH "cd /root/dr-pkg && ls -lh /root/dr-remote-server-20261117.zip && test -s /root/dr-remote-server-20261117.zip && echo SV-ZIP-EXISTS"

echo "== [B2] 拉回远程角色 zip =="
scp -q -i "$KEY" -P 53422 -o ServerAliveInterval=30 -o ServerAliveCountMax=20 "root@sv.trae.show:/root/dr-remote-server-20261117.zip" "$DEST/remote-server.zip"
ls -lh "$DEST/remote-server.zip"
rm -f "$KEY"

echo "== [C] 开发机角色：重建源码包 =="
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
  Remove-Item 'B:\EasyRDP-DR-20261117\dev-machine.zip' -Force -ErrorAction SilentlyContinue
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  [System.IO.Compression.ZipFile]::CreateFromDirectory('B:\EasyRDP-DR-20261117\dev-machine', 'B:\EasyRDP-DR-20261117\dev-machine.zip', [System.IO.Compression.CompressionLevel]::Optimal, \$false)
"
ls -lh "$DEST/dev-machine.zip"

echo "== [E] 清理旧 dr-build 残留 =="
rm -rf /mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/dr-build

echo "== [F] 最终交付清单 =="
ls -lh "$DEST"
echo "[$(date '+%F %T')] ✅ 基线灾备刷新完成（下次刷新：v0.4.0 二级版本发布时）"
