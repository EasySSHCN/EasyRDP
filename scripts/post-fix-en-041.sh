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
SCP en/download.html "$SITE_HOST:$SITE_DIR/en/download.html"
SCP en/changelog.html "$SITE_HOST:$SITE_DIR/en/changelog.html"
SSH "chown www:www $SITE_DIR/en/download.html $SITE_DIR/en/changelog.html; echo chown-ok"
rm -f "$KEY"
echo "=== 线上验证 ==="
D=$(curl -s "https://easyrdp.cn/en/download.html")
echo "$D" | grep -c 'EasyRDP-0.3.4' | xargs echo "0.3.4 残留(应为0):"
echo "$D" | grep -o 'data-dl="EasyRDP-0.4.1-portable.exe"' | head -1
echo "$D" | grep -o "fetch('../api/feedback/top.php'" | head -1
echo "$D" | grep -c 'href="../downloads/' | xargs echo "直链残留(应为0):"
C=$(curl -s "https://easyrdp.cn/en/changelog.html")
echo "$C" | grep -A3 '<h2>v0.4.1</h2>' | grep -o 'Current build' | head -1
echo "$C" | grep -o '<h2>v0.4.0</h2>' | head -1
