#!/usr/bin/env bash
set -e
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
VAULT=$ROOT/secrets-vault
ENC=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$VAULT/site-server.enc" -pass file:"$VAULT/key")
SITE_HOST=$(echo "$ENC" | sed -n 's/^host: \([^ ]*\).*/\1/p')
SITE_PORT=$(echo "$ENC" | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
SITE_DIR=$(echo "$ENC" | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
KEY=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" $KEY && chmod 600 $KEY
cd $ROOT/easyrdp-site
scp -q -i "$KEY" -P "${SITE_PORT:-22}" manual/export-json.html "$SITE_HOST:$SITE_DIR/manual/export-json.html"
scp -q -i "$KEY" -P "${SITE_PORT:-22}" assets/img/manual/export-json.png assets/img/manual/import-json.png assets/img/manual/connections.png assets/img/manual/feedback.png assets/img/manual/groups.png "$SITE_HOST:$SITE_DIR/assets/img/manual/"
ssh -i "$KEY" -p "${SITE_PORT:-22}" "$SITE_HOST" "chown www:www $SITE_DIR/manual/export-json.html $SITE_DIR/assets/img/manual/*.png; echo chown-ok"
rm -f $KEY
for u in manual/export-json.html manual/import-json.html assets/img/manual/export-json.png assets/img/manual/import-json.png assets/img/manual/connections.png assets/img/manual/feedback.png assets/img/manual/groups.png; do
  echo "$u -> $(curl -s -o /dev/null -w '%{http_code}' https://easyrdp.cn/$u)"
done
echo 'xmmaster grep on three pages:'
for p in connections feedback groups; do echo "$p: $(curl -s https://easyrdp.cn/manual/$p.html | grep -c xmmaster || true) (html文字,应为0) $(curl -s https://easyrdp.cn/assets/img/manual/$p.png | grep -c xmmaster || true)"; done
