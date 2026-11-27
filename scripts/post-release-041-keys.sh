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
# 在服务器端生成 keys.php（密钥不出服务器）
SSH "umask 077; K=\$(openssl rand -base64 32); printf '<?php exit(0); ?>\n{\"dl_sign_key\":\"%s\",\"bind_ip\":true,\"check_referer\":true,\"rate_per_min\":10}\n' \"\$K\" > $SITE_DIR/api/download/keys.php; chown www:www $SITE_DIR/api/download/keys.php; chmod 600 $SITE_DIR/api/download/keys.php; echo keys-php-written"
rm -f "$KEY"
echo "=== 验证 token 流（带本站 Referer）==="
T=$(curl -s -X POST "https://easyrdp.cn/api/download/token.php" -H 'Referer: https://easyrdp.cn/download.html' -d 'file=EasyRDP-0.4.1-portable.exe')
echo "token resp: $T"
URL=$(echo "$T" | python3 -c "import json,sys;print(json.load(sys.stdin).get('url',''))" 2>/dev/null)
if [ -n "$URL" ]; then
  curl -s -o /dev/null -w 'GET %s -> %{http_code} (type=%{content_type}, len=%{size_download} 前16KB)\n' -r 0-16383 "https://easyrdp.cn$URL"
  curl -s -o /dev/null -w '白名单外 -> %{http_code}\n' -X POST "https://easyrdp.cn/api/download/token.php" -H 'Referer: https://easyrdp.cn/download.html' -d 'file=evil.exe'
  curl -s -o /dev/null -w '无Referer(头缺失放行) -> %{http_code}\n' -X POST "https://easyrdp.cn/api/download/token.php" -d 'file=EasyRDP-0.4.1-portable.exe'
fi
