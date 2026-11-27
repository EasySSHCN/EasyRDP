#!/usr/bin/env bash
# upload-css.sh —— 上传 style.css（hero 副标题宽度调整）
set -e
SV=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/secrets-vault
dec() { openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key"; }
SITE_HOST=$(dec | sed -n 's/^host: \([^ ]*\).*/\1/p')
SITE_PORT=$(dec | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
SITE_DIR=$(dec | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
KEY=/tmp/k5
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"
chmod 600 "$KEY"
scp -q -i "$KEY" -P "${SITE_PORT:-22}" /mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-site/assets/css/style.css "root@$SITE_HOST:$SITE_DIR/assets/css/style.css"
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" "chown www:www $SITE_DIR/assets/css/style.css"
rm -f "$KEY"
echo "✅ style.css 已上传（刷新页面即生效，CSS 无强缓存头时建议 Ctrl+F5）"
