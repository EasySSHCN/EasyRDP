#!/usr/bin/env bash
# dr-baseline-final.sh —— 基线灾备刷新（两角色，收录今日全部改动）
set -e
LOG=/mnt/b/EasyRDP-DR-20261117/dr-baseline-progress.log
exec > "$LOG" 2>&1
echo "[$(date '+%F %T')] 基线刷新开始"
KEY=/tmp/ksv
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$KEY"; chmod 600 "$KEY"
PW='EasyRDP-DR-20261117'
DEST=/mnt/b/EasyRDP-DR-20261117

echo "== [A] sv 端重建远程角色密码 zip（最新 webroot：EN 站/后台 4 标签/统计 API/beacon）=="
ssh -i "$KEY" -p 53422 -o StrictHostKeyChecking=accept-new -o ServerAliveInterval=30 -o ServerAliveCountMax=20 root@sv.trae.show 'bash -s' <<REMOTE
set -e
command -v zip >/dev/null 2>&1 || dnf install -y zip >/dev/null
R=/www/wwwroot/easyrdp.cn
P=/root/dr-pkg/remote-server
rm -rf "\$P"; mkdir -p "\$P"
cd "\$R"
tar -czf "\$P/webroot.tar.gz" --exclude='downloads/SHA256SUMS.txt.bak' .
echo "-- webroot tar 完成 --"; date +%T
cp api/feedback/keys.php "\$P/keys.php"
tar -czf "\$P/easyrdp-data.tar.gz" -C /www/wwwroot easyrdp-data
cd /root/dr-pkg
rm -f /root/dr-remote-server-20261117.zip
echo "-- zip 开始 --"; date +%T
zip -r -q -P '$PW' /root/dr-remote-server-20261117.zip remote-server
echo "-- zip 完成 --"; date +%T
unzip -t /root/dr-remote-server-20261117.zip > /dev/null && echo "SV-ZIP-OK"
REMOTE
echo "== [B] 拉回远程角色 zip =="
mkdir -p "$DEST"
EK=/tmp/kek2
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"
scp -q -i "$EK" -P 53422 "root@sv.trae.show:/root/dr-remote-server-20261117.zip" "$DEST/remote-server.zip"
ssh -i "$EK" -p 53422 root@sv.trae.show 'ls -lh /root/dr-pkg/remote-server/ /root/dr-remote-server-20261117.zip'
rm -f "$EK"

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
  Remove-Item 'B:\EasyRDP-DR-20261117\dev-machine.zip' -Force -ErrorAction SilentlyContinue
  Add-Type -AssemblyName System.IO.Compression.FileSystem
  [System.IO.Compression.ZipFile]::CreateFromDirectory('B:\EasyRDP-DR-20261117\dev-machine', 'B:\EasyRDP-DR-20261117\dev-machine.zip', [System.IO.Compression.CompressionLevel]::Optimal, \$false)
"
Get-Item 'B:\EasyRDP-DR-20261117\dev-machine.zip' | Select Length,LastWriteTime | Out-String -Width 120

echo "== [E] 清理旧 dr-build 残留目录（已迁移至 B:\EasyRDP-DR-20261117）=="
rm -rf /mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/dr-build

echo "== [F] 最终交付清单 =="
ls -lh "$DEST"
Get-ChildItem "$DEST" -File | ForEach-Object { "  {0}  {1:N0} bytes" -f $_.Name, $_.Length } | Out-String -Width 120
echo "✅ 基线灾备刷新完成（下次刷新：v0.4.0 二级版本发布时）"
