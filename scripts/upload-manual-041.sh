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
SSH "mkdir -p $SITE_DIR/manual"
cd "$ROOT/easyrdp-site/manual"
for f in *.html; do SCP "$f" "$SITE_HOST:$SITE_DIR/manual/$f"; done
SSH "chown www:www $SITE_DIR/manual/*.html; ls $SITE_DIR/manual | wc -l"
rm -f "$KEY"
echo "=== 逐页线上验证 ==="
for p in index connections settings feedback import-rdp groups export import-json templates fullscreen win-key disconnect git-sync-no-git; do
  printf "%-18s %s\n" "$p" "$(curl -s -o /dev/null -w '%{http_code}' "https://easyrdp.cn/manual/$p.html")"
done
