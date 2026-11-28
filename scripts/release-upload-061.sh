#!/usr/bin/env bash
# release-upload-061.sh — v0.6.1 发布资产与官网页面上传 + 线上校验
set -e
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
SV=$ROOT/secrets-vault
D=$ROOT/easyrdp-client/dist
SITE=$ROOT/easyrdp-site
CFG=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key")
H=$(echo "$CFG" | sed -n 's/^host: \([^ ]*\).*/\1/p')
PORT=$(echo "$CFG" | sed -n 's/^ssh: .*port \([0-9]*\).*/\1/p')
DIR=$(echo "$CFG" | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" /tmp/id_ed25519_easyssh
chmod 600 /tmp/id_ed25519_easyssh
SCP="scp -q -i /tmp/id_ed25519_easyssh -P ${PORT:-22}"
SSH="ssh -q -i /tmp/id_ed25519_easyssh -p ${PORT:-22}"

echo "== 1) 下载资产 =="
$SCP "$D/EasyRDP-0.6.1-portable.exe" "$D/EasyRDP-0.6.1-win32-x64-portable.zip" \
     "$ROOT/dist-gateway/EasyRDP-Gateway-0.6.1-win-x64.zip" \
     "$ROOT/EasyRDP-src-0.6.1.tar.gz" "$D/SHA256SUMS.txt" \
     "root@$H:$DIR/downloads/"

echo "== 2) 页面与静态资源 =="
$SCP "$SITE/download.html" "$SITE/changelog.html" "root@$H:$DIR/"
$SCP "$SITE/en/download.html" "root@$H:$DIR/en/"
$SCP "$SITE/assets/js/main.js" "root@$H:$DIR/assets/js/"
$SCP "$SITE/assets/data/site-updates.json" "root@$H:$DIR/assets/data/"
$SSH "root@$H" "chown -R www:www $DIR/downloads/EasyRDP-0.6.1* $DIR/downloads/SHA256SUMS.txt $DIR/download.html $DIR/changelog.html $DIR/en/download.html $DIR/assets/js/main.js $DIR/assets/data/site-updates.json && echo chown-ok"
rm -f /tmp/id_ed25519_easyssh

echo "== 3) 线上校验 =="
for u in \
  "https://easyrdp.cn/downloads/EasyRDP-0.6.1-portable.exe" \
  "https://easyrdp.cn/downloads/EasyRDP-0.6.1-win32-x64-portable.zip" \
  "https://easyrdp.cn/downloads/EasyRDP-Gateway-0.6.1-win-x64.zip" \
  "https://easyrdp.cn/downloads/EasyRDP-src-0.6.1.tar.gz" \
  "https://easyrdp.cn/downloads/SHA256SUMS.txt" \
  "https://easyrdp.cn/download.html" \
  "https://easyrdp.cn/changelog.html"; do
  CODE=$(curl -s -o /dev/null -w "%{http_code}" "$u")
  echo "$CODE  $u"
done
