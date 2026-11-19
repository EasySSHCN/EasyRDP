#!/usr/bin/env bash
# upload-index-shot.sh —— 上传首页改版与截图资产
set -e
SV=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/secrets-vault
dec() { openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key"; }
SITE_HOST=$(dec | sed -n 's/^host: \([^ ]*\).*/\1/p')
SITE_PORT=$(dec | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
SITE_DIR=$(dec | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
SITE=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-site
KEY=/tmp/k4
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"
chmod 600 "$KEY"
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" "mkdir -p $SITE_DIR/assets/img"
scp -q -i "$KEY" -P "${SITE_PORT:-22}" "$SITE/index.html" "root@$SITE_HOST:$SITE_DIR/index.html"
scp -q -i "$KEY" -P "${SITE_PORT:-22}" "$SITE/assets/img/app-screenshot.webp" "$SITE/assets/img/app-screenshot.png" "root@$SITE_HOST:$SITE_DIR/assets/img/"
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" "chown www:www $SITE_DIR/index.html $SITE_DIR/assets/img/app-screenshot.webp $SITE_DIR/assets/img/app-screenshot.png"
rm -f "$KEY"
sleep 2
IX=$(curl -s "https://easyrdp.cn/index.html?nocache=$RANDOM")
echo "线上含 shot 区块: $(echo "$IX" | grep -c 'class="shot"')"
echo "线上含 webp 引用: $(echo "$IX" | grep -c 'app-screenshot.webp')"
curl -s -o /dev/null -w "webp HTTP %{http_code} %{size_download}B\n" "https://easyrdp.cn/assets/img/app-screenshot.webp?nocache=$RANDOM"
