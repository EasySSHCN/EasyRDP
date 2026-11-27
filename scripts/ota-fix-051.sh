#!/usr/bin/env bash
# ota-fix-051.sh —— v0.5.1 资产修复上传（官网 downloads 五件 + OTA）
set -e
SV=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/secrets-vault
SITE=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key")
H=$(echo "$SITE" | sed -n 's/^host: \([^ ]*\).*/\1/p')
PORT=$(echo "$SITE" | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
DIR=$(echo "$SITE" | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" /tmp/id_ed
chmod 600 /tmp/id_ed
R=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
D=$R/easyrdp-client/dist
scp -q -i /tmp/id_ed -P "${PORT:-22}" \
  "$D/EasyRDP-0.5.1-portable.exe" \
  "$D/EasyRDP-0.5.1-win32-x64-portable.zip" \
  "$D/SHA256SUMS.txt" \
  "root@$H:$DIR/downloads/"
scp -q -i /tmp/id_ed -P "${PORT:-22}" \
  "$R/easyrdp-site/downloads/EasyRDP-0.5.1-portable.exe.enc" \
  "root@$H:$DIR/downloads/"
scp -q -i /tmp/id_ed -P "${PORT:-22}" \
  "$R/easyrdp-site/api/update/latest.json" \
  "root@$H:$DIR/api/update/latest.json"
ssh -q -i /tmp/id_ed -p "${PORT:-22}" "root@$H" "chown www:www $DIR/downloads/EasyRDP-0.5.1-portable.exe $DIR/downloads/EasyRDP-0.5.1-win32-x64-portable.zip $DIR/downloads/SHA256SUMS.txt $DIR/downloads/EasyRDP-0.5.1-portable.exe.enc $DIR/api/update/latest.json; sha256sum $DIR/downloads/EasyRDP-0.5.1-portable.exe | cut -c1-16"
rm -f /tmp/id_ed
echo "==> 线上复验"
curl -s "https://easyrdp.cn/api/update/latest.json?nocache=$RANDOM" | head -c 120; echo
curl -s -o /dev/null -w "portable HTTP %{http_code}\n" "https://easyrdp.cn/downloads/EasyRDP-0.5.1-portable.exe"
curl -s "https://easyrdp.cn/downloads/SHA256SUMS.txt" | head -2
