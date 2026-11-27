#!/usr/bin/env bash
# Upload OTA latest.json + .enc for v0.4.3 and verify online
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENC="$ROOT/secrets-vault/site-server.enc"; KEYF="$ROOT/secrets-vault/key"
SITE_HOST=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$ENC" -pass file:"$KEYF" | sed -n 's/^host: \([^ ]*\).*/\1/p')
SITE_PORT=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$ENC" -pass file:"$KEYF" | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
SITE_DIR=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$ENC" -pass file:"$KEYF" | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
KEY_PATH=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY_PATH" && chmod 600 "$KEY_PATH" || exit 1
trap 'rm -f "$KEY_PATH"' EXIT
ssh -i "$KEY_PATH" -p "${SITE_PORT:-22}" "$SITE_HOST" "mkdir -p $SITE_DIR/api/update $SITE_DIR/downloads"
scp -q -i "$KEY_PATH" -P "${SITE_PORT:-22}" "$ROOT/easyrdp-site/api/update/latest.json" "$SITE_HOST:$SITE_DIR/api/update/latest.json" || exit 1
scp -q -i "$KEY_PATH" -P "${SITE_PORT:-22}" "$ROOT/easyrdp-site/downloads/EasyRDP-0.4.3-portable.exe.enc" "$SITE_HOST:$SITE_DIR/downloads/" || exit 1
ssh -i "$KEY_PATH" -p "${SITE_PORT:-22}" "$SITE_HOST" "chown www:www $SITE_DIR/api/update/latest.json $SITE_DIR/downloads/EasyRDP-0.4.3-portable.exe.enc"
CODE=$(curl -s -o /dev/null -w '%{http_code}' https://easyrdp.cn/api/update/latest.json)
echo "latest.json HTTP $CODE"
curl -s https://easyrdp.cn/api/update/latest.json | head -c 200; echo
