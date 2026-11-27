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
SSH "mkdir -p $SITE_DIR/assets/img/manual"
cd "$ROOT/easyrdp-site"
for f in manual/*.html; do SCP "$f" "$SITE_HOST:$SITE_DIR/$f"; done
for f in assets/img/manual/*.png; do SCP "$f" "$SITE_HOST:$SITE_DIR/$f"; done
SSH "chown -R www:www $SITE_DIR/manual $SITE_DIR/assets/img/manual; echo uploaded"
rm -f "$KEY"
echo "=== 逐页验证（含图片 200）==="
for p in connections settings feedback import-rdp groups export import-json templates; do
  HTML=$(curl -s -o /dev/null -w '%{http_code}' "https://easyrdp.cn/manual/$p.html")
  IMG=$(curl -s -o /dev/null -w '%{http_code}' "https://easyrdp.cn/assets/img/manual/$p.png")
  echo "$p.html=$HTML $p.png=$IMG"
done
curl -s "https://easyrdp.cn/manual/connections.html?nocache=$RANDOM" | grep -c '截图占位' | xargs echo "connections 残留占位(应为0):"
