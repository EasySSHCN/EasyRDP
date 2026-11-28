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
# 1) OTA 产物
SCP api/update/latest.json "$SITE_HOST:$SITE_DIR/api/update/latest.json"
SCP downloads/EasyRDP-0.4.1-portable.exe.enc "$SITE_HOST:$SITE_DIR/downloads/EasyRDP-0.4.1-portable.exe.enc"
echo "OTA 产物已上传"
# 2) 下载 Token 化接口 + 统计归因
SSH "mkdir -p $SITE_DIR/api/download/token"
for f in api/download/common.php api/download/token.php api/download/get.php api/download/index.php api/download/token/index.php api/download/keys.example.php api/stats/client.php; do
  SCP "$f" "$SITE_HOST:$SITE_DIR/$f" || echo "FAIL $f"
done
echo "api PHP 已上传"
# 3) manual 新页 + en 页面
SSH "mkdir -p $SITE_DIR/manual"
SCP manual/git-sync-no-git.html "$SITE_HOST:$SITE_DIR/manual/git-sync-no-git.html"
SCP manual/index.html "$SITE_HOST:$SITE_DIR/manual/index.html"
for f in en/*.html; do SCP "$f" "$SITE_HOST:$SITE_DIR/$f"; done
echo "manual/en 页面已上传"
# 4) 服务器端清理死文件 + chown
SSH "rm -f $SITE_DIR/assets/js/visit-beacon.js; chown www:www $SITE_DIR/downloads/EasyRDP-0.4.1-portable.exe $SITE_DIR/downloads/EasyRDP-0.4.1-win32-x64-portable.zip $SITE_DIR/downloads/EasyRDP-Gateway-0.4.1-win-x64.zip $SITE_DIR/downloads/EasyRDP-src-0.4.1.tar.gz $SITE_DIR/downloads/SHA256SUMS.txt $SITE_DIR/downloads/EasyRDP-0.4.1-portable.exe.enc; chown -R www:www $SITE_DIR/api/download $SITE_DIR/api/stats/client.php $SITE_DIR/api/update/latest.json $SITE_DIR/manual/git-sync-no-git.html $SITE_DIR/manual/index.html; echo chown-ok"
rm -f "$KEY"
echo "=== 线上验证 ==="
for u in "https://easyrdp.cn/manual/git-sync-no-git.html" "https://easyrdp.cn/downloads/EasyRDP-0.4.1-portable.exe.enc" "https://easyrdp.cn/en/download.html" "https://easyrdp.cn/downloads/EasyRDP-0.4.1-portable.exe"; do
  echo "$u -> $(curl -s -o /dev/null -w '%{http_code}' "$u")"
done
echo "token 接口（无 key 应 500，白名单校验应 403/500 而非 404）:"
curl -s -o /dev/null -w 'POST token.php -> %{http_code}\n' -X POST "https://easyrdp.cn/api/download/token.php" -d 'file=EasyRDP-0.4.1-portable.exe'
curl -s -o /dev/null -w 'GET download 无token -> %{http_code}\n' "https://easyrdp.cn/api/download"
