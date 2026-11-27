#!/usr/bin/env bash
# upload-admin-download.sh —— 上传 admin.html 与 download.html（FILEDL-CARDS/ADMINUI 改版）
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
scp -q -i "$KEY" -P "${SITE_PORT:-22}" "$ROOT/easyrdp-site/download.html" "root@$SITE_HOST:$SITE_DIR/download.html"
scp -q -i "$KEY" -P "${SITE_PORT:-22}" "$ROOT/easyrdp-site/admin.html" "root@$SITE_HOST:$SITE_DIR/admin.html"
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" "chown www:www $SITE_DIR/download.html $SITE_DIR/admin.html && ls -l $SITE_DIR/download.html $SITE_DIR/admin.html"
rm -f "$KEY"
sleep 2
DL=$(curl -s "https://easyrdp.cn/download.html?nocache=$RANDOM")
AD=$(curl -s "https://easyrdp.cn/admin.html?nocache=$RANDOM")
echo "线上 download 卡片: $(echo "$DL" | grep -c 'dl-card')"
echo "线上 download SHA 占位: $(echo "$DL" | grep -c 'data-sha')"
echo "线上 admin 页头: $(echo "$AD" | grep -c 'site-header')"
echo "线上 admin 背景色旧值残留: $(echo "$AD" | grep -c '#0\.3\.30')"
echo "线上 admin 新背景色: $(echo "$AD" | grep -c '#0f1117')"
