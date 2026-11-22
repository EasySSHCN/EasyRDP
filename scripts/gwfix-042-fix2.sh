#!/usr/bin/env bash
# GW-FIX-V042 修正：二进制落位 unit 路径 + [auth] 段内插入开关 + users.db 路径适配 open_basedir
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KEY_PATH=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY_PATH" && chmod 600 "$KEY_PATH" || { echo "✗ SSH 密钥准备失败"; exit 1; }
trap 'rm -f "$KEY_PATH"' EXIT
ENC="$ROOT/secrets-vault/site-server.enc"; KEYF="$ROOT/secrets-vault/key"
SITE_HOST=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$ENC" -pass file:"$KEYF" | sed -n 's/^host: \([^ ]*\).*/\1/p')
SITE_PORT=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$ENC" -pass file:"$KEYF" | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
SITE_DIR=$(openssl enc -aes-256-cbc -d -pbkdf2 -in "$ENC" -pass file:"$KEYF" | sed -n 's|^webroot: \([^ ]*\).*|\1|p')

echo "==> unit ExecStart 确认"
ssh -i "$KEY_PATH" -p "${SITE_PORT:-22}" "$SITE_HOST" "systemctl cat easyrdp-gateway | grep -E 'ExecStart|WorkingDirectory'"

echo "==> 新二进制落位 + config 修正 + 重启"
ssh -i "$KEY_PATH" -p "${SITE_PORT:-22}" "$SITE_HOST" 'set -e
  NEWBIN=$(find /tmp/gw042 -type f -name "*gateway*" -perm -u+x | head -1)
  [ -n "$NEWBIN" ] || NEWBIN=$(find /tmp/gw042 -type f | grep -v "\.toml\|\.md\|\.example" | head -1)
  echo "NEWBIN=$NEWBIN"
  UNIT_BIN=$(systemctl cat easyrdp-gateway | sed -n "s/^ExecStart=//p" | awk "{print \$1}")
  echo "UNIT_BIN=$UNIT_BIN"
  install -m 755 "$NEWBIN" "$UNIT_BIN"
  # 清理我此前误追加的重复 [auth] 段（若存在），再在首个 [auth] 段内插入开关
  CFG=/opt/easyrdp-gateway/config.toml
  python3 - <<PYEOF
import re
p = "$CFG"
s = open(p).read()
# 去掉文件尾部误追加的重复段
s = re.sub(r"\n*\[auth\]\nallow_placeholder_login = false\n*$", "\n", s)
if "allow_placeholder_login" not in s:
    s = re.sub(r"(\[auth\]\n)", r"\1allow_placeholder_login = false\n", s, count=1)
open(p, "w").write(s)
print("config updated")
PYEOF
  grep -A3 "^\[auth\]" "$CFG" | head -5
  systemctl restart easyrdp-gateway
  sleep 1
  systemctl is-active easyrdp-gateway
  ss -ltnp | grep 8443 | head -2
'

echo "==> issue.php 重传（users_db 默认路径适配 open_basedir）"
scp -q -i "$KEY_PATH" -P "${SITE_PORT:-22}" "$ROOT/easyrdp-site/api/gateway/issue.php" "$SITE_HOST:$SITE_DIR/api/gateway/issue.php" \
  && ssh -i "$KEY_PATH" -p "${SITE_PORT:-22}" "$SITE_HOST" "chown www:www $SITE_DIR/api/gateway/issue.php" \
  || { echo "✗ issue.php 重传失败"; exit 1; }

echo "==> users.db 目录（open_basedir 白名单内：/www/wwwroot/easyrdp-data）"
ssh -i "$KEY_PATH" -p "${SITE_PORT:-22}" "$SITE_HOST" "mkdir -p /www/wwwroot/easyrdp-data && chown www:www /www/wwwroot/easyrdp-data && chmod 750 /www/wwwroot/easyrdp-data && ls -ld /www/wwwroot/easyrdp-data"
