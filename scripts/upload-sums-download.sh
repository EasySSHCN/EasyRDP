#!/usr/bin/env bash
# upload-sums-download.sh —— 上传 0.3.3 SUMS 与新版 download.html
set -e
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
SV=$ROOT/secrets-vault
dec() { openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key"; }
SITE_HOST=$(dec | sed -n 's/^host: \([^ ]*\).*/\1/p')
SITE_PORT=$(dec | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
SITE_DIR=$(dec | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
KEY=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"
chmod 600 "$KEY"
scp -q -i "$KEY" -P "${SITE_PORT:-22}" "$ROOT/easyrdp-client/dist/SHA256SUMS.txt" "root@$SITE_HOST:$SITE_DIR/downloads/SHA256SUMS.txt"
scp -q -i "$KEY" -P "${SITE_PORT:-22}" "$ROOT/easyrdp-site/download.html" "root@$SITE_HOST:$SITE_DIR/download.html"
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" "chown www:www $SITE_DIR/downloads/SHA256SUMS.txt $SITE_DIR/download.html"
rm -f "$KEY"
sleep 2
SUMS=$(curl -s "https://easyrdp.cn/downloads/SHA256SUMS.txt?nocache=$RANDOM")
echo "$SUMS" | head -2
DL=$(curl -s "https://easyrdp.cn/download.html?nocache=$RANDOM")
echo "线上含历史版本表: $(echo "$DL" | grep -c '历史版本')"
echo "线上含容器包裹卡片: $(echo "$DL" | grep -c 'dl-cards')"
