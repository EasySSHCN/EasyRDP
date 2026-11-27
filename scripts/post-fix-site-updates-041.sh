#!/usr/bin/env bash
set -e
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
cd "$ROOT"
# node 在 Windows 侧执行（site-updates.json 已生成：current=1.6.0）
[ -f easyrdp-site/assets/data/site-updates.json ] || { echo "site-updates.json missing"; exit 1; }
head -c 120 easyrdp-site/assets/data/site-updates.json; echo
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
SCP download.html "$SITE_HOST:$SITE_DIR/download.html"
SSH "mkdir -p $SITE_DIR/assets/data"
SCP assets/data/site-updates.json "$SITE_HOST:$SITE_DIR/assets/data/site-updates.json"
SSH "chown www:www $SITE_DIR/download.html $SITE_DIR/assets/data/site-updates.json; echo uploaded"
echo "== 服务器 404 页清理 =="
SSH "grep -rl 'visit-beacon\|api/visit/track' $SITE_DIR --include='*.html' 2>/dev/null | head -5"
SSH "F=\$(grep -rl 'visit-beacon' $SITE_DIR --include='*.html' 2>/dev/null | head -1); if [ -n \"\$F\" ]; then sed -i 's|<script src=\"assets/js/visit-beacon.js\" async></script>||g; s|<script src=\"[^\"]*visit-beacon.js\"[^>]*></script>||g' \$F; chown www:www \$F; echo cleaned: \$F; else echo no-404-ref-found; fi"
rm -f "$KEY"
echo "=== 线上复验 ==="
curl -s -o /dev/null -w 'site-updates.json -> %{http_code}\n' "https://easyrdp.cn/assets/data/site-updates.json?nocache=$RANDOM"
D=$(curl -s "https://easyrdp.cn/download.html?nocache=$RANDOM")
echo "$D" | grep -o '2026-11-20 发布 v0.4.1' | head -1
echo "$D" | grep -c 'data-dl="EasyRDP-0.4.1-portable.exe"' | xargs echo "0.4.1 卡片链接数:"
echo "$D" | grep -o 'data-dl="EasyRDP-0.4.0-portable.exe"' | head -1
curl -s "https://easyrdp.cn/nonexist-page-$RANDOM.html" | grep -c 'visit-beacon' | xargs echo "404页 beacon 残留(应为0):"
