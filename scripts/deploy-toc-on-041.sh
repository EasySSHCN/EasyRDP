#!/usr/bin/env bash
set -e
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
VAULT=$ROOT/secrets-vault
ENC=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VAULT/site-server.enc" -pass file:"$VAULT/key")
SITE_HOST=$(echo "$ENC" | sed -n 's/^host: \([^ ]*\).*/\1/p')
SITE_PORT=$(echo "$ENC" | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
SITE_DIR=$(echo "$ENC" | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
KEY=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY" && chmod 600 "$KEY"
SSH() { ssh -i "$KEY" -p "${SITE_PORT:-22}" -o StrictHostKeyChecking=accept-new "$SITE_HOST" "$@"; }
SCP() { scp -q -i "$KEY" -P "${SITE_PORT:-22}" -o StrictHostKeyChecking=accept-new "$@"; }
cd "$ROOT/easyrdp-site"
SCP assets/js/main.js "$SITE_HOST:$SITE_DIR/assets/js/main.js"
SCP assets/css/style.css "$SITE_HOST:$SITE_DIR/assets/css/style.css"
for f in *.html; do SCP "$f" "$SITE_HOST:$SITE_DIR/$f"; done
for f in manual/*.html en/*.html; do SCP "$f" "$SITE_HOST:$SITE_DIR/$f"; done
SSH "chown www:www $SITE_DIR/assets/js/main.js $SITE_DIR/assets/css/style.css $SITE_DIR/*.html $SITE_DIR/en $SITE_DIR/manual; echo chown-ok"
rm -f "$KEY"
echo "=== 验证 ==="
curl -s -o /dev/null -w 'main.js?v=25 %{http_code}\n' "https://easyrdp.cn/assets/js/main.js?v=25&nocache=$RANDOM"
curl -s "https://easyrdp.cn/assets/js/main.js?nocache=$RANDOM" | grep -c 'TOC-ON 20261120' | xargs echo 'main.js TOC-ON:'
curl -s "https://easyrdp.cn/assets/css/style.css?nocache=$RANDOM" | grep -c 'doc-toc a.on' | xargs echo 'style.css .on:'
for pg in index connections settings export export-json; do
  C=$(curl -s "https://easyrdp.cn/manual/$pg.html?nocache=$RANDOM")
  echo "$pg: v25=$(echo "$C" | grep -c 'main.js?v=25') v24残留=$(echo "$C" | grep -c 'main.js?v=24')"
done
