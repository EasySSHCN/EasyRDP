#!/usr/bin/env bash
# dr-refresh-and-repack.sh —— 刷新灾备包内容（从 sv 增量拉取改版文件）并重新打包
set -e
KEY=/tmp/kpull
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$KEY"; chmod 600 "$KEY"
SV="root@sv.trae.show"
OPTS="ssh -i $KEY -p 53422 -o StrictHostKeyChecking=accept-new -o BatchMode=yes"
DR=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/dr-build

echo "== [1] 增量拉取改版文件（admin/stats/site-updates/beacon）=="
rsync -az --partial -e "$OPTS" \
  --include 'admin.html' \
  --include 'api/***' \
  --include 'assets/data/***' \
  --include 'assets/js/***' \
  --exclude '*' \
  "$SV:/www/wwwroot/easyrdp.cn/" "$DR/remote-server/webroot/"
rsync -az -e "$OPTS" "root@sv.trae.show:/www/wwwroot/easyrdp.cn/tools/gen-site-updates.js" "$DR/remote-server/webroot/tools/gen-site-updates.js" 2>/dev/null || \
  mkdir -p "$DR/remote-server/webroot/tools"

echo "== [2] 重建开发角色源码包（含最新 easyrdp-site/crates 源码）=="
cd /mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace
rm -f EasyRDP/dr-build/dev-machine/EasyRDP.tar.gz
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

echo "== [3] 重新打包灾备压缩包 =="
rm -f /mnt/b/EasyRDP-DR-20261117-roles.tar.gz
tar -czf /mnt/b/EasyRDP-DR-20261117-roles.tar.gz \
  --exclude 'dr-build/dev-machine/tools/EasyRDP/vcpkg/downloads' \
  --exclude 'dr-pull-work' \
  -C dr-build .
ls -lh /mnt/b/EasyRDP-DR-20261117-roles.tar.gz
rm -f "$KEY"
echo "✅ 灾备包刷新完成"
