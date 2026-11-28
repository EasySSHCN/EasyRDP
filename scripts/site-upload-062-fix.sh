#!/usr/bin/env bash
# site-upload-062-fix.sh — 重建产物五件套上传官网 downloads
set -e
R=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
SV=$R/secrets-vault
CFG=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key")
H=$(echo "$CFG" | sed -n 's/^host: \([^ ]*\).*/\1/p')
PORT=$(echo "$CFG" | sed -n 's/^ssh: .*port \([0-9]*\).*/\1/p')
DIR=$(echo "$CFG" | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
echo "host=$H port=$PORT dir=$DIR"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" /tmp/id_ed25519_easyssh
chmod 600 /tmp/id_ed25519_easyssh
scp -q -i /tmp/id_ed25519_easyssh -P "${PORT:-22}" \
  "$R/easyrdp-client/dist/EasyRDP-0.6.2-portable.exe" \
  "$R/easyrdp-client/dist/EasyRDP-0.6.2-win32-x64-portable.zip" \
  "$R/dist-gateway/EasyRDP-Gateway-0.6.2-win-x64.zip" \
  "$R/EasyRDP-src-0.6.2.tar.gz" \
  "$R/easyrdp-client/dist/SHA256SUMS.txt" \
  "root@$H:$DIR/downloads/"
ssh -q -i /tmp/id_ed25519_easyssh -p "${PORT:-22}" "root@$H" \
  "chown www:www $DIR/downloads/EasyRDP-0.6.2* $DIR/downloads/SHA256SUMS.txt && sha256sum $DIR/downloads/EasyRDP-0.6.2-portable.exe | cut -c1-16 && echo upload-ok"
rm -f /tmp/id_ed25519_easyssh
