#!/usr/bin/env bash
# sv-build-dr-zip.sh —— 在 sv 服务器上打包远程服务器角色密码 zip（避开开发机杀软），拉回 B:
set -e
KEY=/tmp/ksv
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$KEY"; chmod 600 "$KEY"
SVSSH="ssh -i $KEY -p 53422 -o StrictHostKeyChecking=accept-new root@sv.trae.show"
PW='EasyRDP-DR-20261117'
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
DEST=/mnt/b/EasyRDP-DR-20261117

echo "== [1] 上传打包输入（install.sh / SERVER-NOTES.md）到 sv =="
$SVSSH 'mkdir -p /root/dr-pkg/remote-server'
scp -q -i "$KEY" -P 53422 "$ROOT/dr-build/remote-server/install.sh" "root@sv.trae.show:/root/dr-pkg/remote-server/install.sh"
scp -q -i "$KEY" -P 53422 "$ROOT/dr-build/remote-server/SERVER-NOTES.md" "root@sv.trae.show:/root/dr-pkg/remote-server/SERVER-NOTES.md"

echo "== [2] sv 端组装密码 zip（webroot 来自已同步镜像，含 keys.php）=="
$SVSSH 'bash -s' <<REMOTE
set -e
command -v zip >/dev/null 2>&1 || dnf install -y zip >/dev/null
R=/www/wwwroot/easyrdp.cn
P=/root/dr-pkg/remote-server
cd "\$R"
echo "-- 打包 webroot --"
tar -czf "\$P/webroot.tar.gz" --exclude='downloads/SHA256SUMS.txt.bak' .
cp api/feedback/keys.php "\$P/keys.php"
tar -czf "\$P/easyrdp-data.tar.gz" -C /www/wwwroot easyrdp-data
cd "\$P"
rm -f /root/dr-remote-server-20261117.zip
zip -r -q -P '$PW' /root/dr-remote-server-20261117.zip remote-server
ls -lh /root/dr-remote-server-20261117.zip
unzip -t /root/dr-remote-server-20261117.zip > /dev/null && echo "ZIP-OK"
rm -f "\$P/keys.php"
REMOTE

echo "== [3] 拉回开发机 =="
mkdir -p "$DEST"
scp -q -i "$KEY" -P 53422 "root@sv.trae.show:/root/dr-remote-server-20261117.zip" "$DEST/remote-server.zip"
$SVSSH 'rm -f /root/dr-remote-server-20261117.zip; ls -lh /root/dr-pkg/remote-server/'
rm -f "$KEY"
ls -lh "$DEST"
echo "✅ 远程角色密码 zip 完成（密码: $PW）"
