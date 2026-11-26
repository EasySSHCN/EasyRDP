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
for f in api/stats/common.php api/feedback/submit.php; do
  SCP "$f" "$SITE_HOST:$SITE_DIR/$f"
  SSH "chown www:www $SITE_DIR/$f"
done
rm -f "$KEY"
echo "=== 复测 token 全流程 ==="
T=$(curl -s -X POST "https://easyrdp.cn/api/download/token.php" -H 'Referer: https://easyrdp.cn/download.html' -d 'file=EasyRDP-0.4.1-portable.exe')
echo "token resp: $T"
URL=$(echo "$T" | python3 -c "import json,sys;print(json.load(sys.stdin).get('url',''))")
echo "parsed url: $URL"
curl -s -o /dev/null -w 'Range 前16KB -> %{http_code} type=%{content_type}\n' -r 0-16383 "https://easyrdp.cn$URL"
curl -s -o /dev/null -w '白名单外 -> %{http_code}\n' -X POST "https://easyrdp.cn/api/download/token.php" -H 'Referer: https://easyrdp.cn/download.html' -d 'file=evil.exe'
curl -s -o /dev/null -w '无Referer(放行) -> %{http_code}\n' -X POST "https://easyrdp.cn/api/download/token.php" -d 'file=EasyRDP-0.4.1-portable.exe'
