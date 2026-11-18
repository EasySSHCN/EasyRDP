#!/usr/bin/env bash
set -e
LOG=/mnt/b/EasyRDP-DR-20261117/dr-baseline-progress.log
exec >> "$LOG" 2>&1
echo "[$(date '+%F %T')] 改从 198 拉取远程角色内容（sv 链路不稳）"
KEY=/tmp/k198
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"; chmod 600 "$KEY"
W=/mnt/b/EasyRDP-DR-20261117/pull-198
rm -rf "$W"; mkdir -p "$W/webroot"
rsync -a --timeout=180 -e "ssh -i $KEY -p 22" root@198.2.231.95:/www/wwwroot/easyrdp.cn/ "$W/webroot/"
echo "[$(date '+%T')] webroot rsync 完成"
scp -q -i "$KEY" -P 22 root@198.2.231.95:/www/wwwroot/easyrdp.cn/api/feedback/keys.php "$W/keys.php"
rsync -a --timeout=180 -e "ssh -i $KEY -p 22" root@198.2.231.95:/www/wwwroot/easyrdp-data/ "$W/easyrdp-data/"
rm -f "$KEY"

echo "[$(date '+%T')] 本地打密码 zip"
cd "$W"
rm -f ../remote-server.zip
zip -r -q -P 'EasyRDP-DR-20261117' ../remote-server.zip webroot keys.php easyrdp-data
ls -lh ../remote-server.zip
unzip -l -P 'EasyRDP-DR-20261117' ../remote-server.zip | grep -cE 'webroot|keys|easyrdp-data'
unzip -t -P 'EasyRDP-DR-20261117' ../remote-server.zip > /dev/null && echo "LOCAL-ZIP-OK"
echo "[$(date '+%T')] ✅ 远程角色 zip（198 源）完成"
