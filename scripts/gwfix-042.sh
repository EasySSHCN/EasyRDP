#!/usr/bin/env bash
# GW-FIX-V042 —— v0.4.2 网关修复包服务器侧部署（RELEASE.md GW-FIX-V042 清单②③④⑤）
# 用法：wsl bash scripts/gwfix-042.sh（在 release.sh 0.4.2 完成后运行）
set -u
KEY_PATH=/tmp/id_ed25519_easyssh
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[ -f "$ROOT/scripts/release.sh" ] || ROOT="$(pwd)"
GWZIP="$ROOT/dist-gateway/EasyRDP-Gateway-0.4.2-win-x64.zip"
[ -f "$GWZIP" ] || { echo "✗ 缺 $GWZIP（先跑 release.sh 0.4.2）"; exit 1; }

ENC="$ROOT/secrets-vault/site-server.enc"
KEYF="$ROOT/secrets-vault/key"
DEC() { openssl enc -aes-256-cbc -d -pbkdf2 -in "$ENC" -pass file:"$KEYF"; }
SITE_HOST=$(DEC | sed -n 's/^host: \([^ ]*\).*/\1/p')
SITE_PORT=$(DEC | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
SITE_DIR=$(DEC | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
[ -n "$SITE_HOST" ] || { echo "✗ 服务器信息解密失败"; exit 1; }
SSH="ssh -i $KEY_PATH -o StrictHostKeyChecking=accept-new -p ${SITE_PORT:-22} $SITE_HOST"
SCP="scp -i $KEY_PATH -o StrictHostKeyChecking=accept-new -P ${SITE_PORT:-22}"
KEY_PATH=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY_PATH" && chmod 600 "$KEY_PATH" || { echo "✗ SSH 密钥准备失败"; exit 1; }
trap 'rm -f "$KEY_PATH"' EXIT

echo "==> ② 网关二进制升级"
$SSH 'uname -s; ls /opt/easyrdp-gateway 2>/dev/null; systemctl list-units --type=service --no-pager 2>/dev/null | grep -i easyrdp || true'
$SCP "$GWZIP" "$SITE_HOST:/tmp/gw-042.zip" || { echo "✗ 网关 zip 上传失败"; exit 1; }
$SSH 'set -e
  cd /tmp && rm -rf gw042 && mkdir gw042 && cd gw042 && unzip -o -q /tmp/gw-042.zip
  EXE=$(find . -name "easyrdp-gateway*" -type f -perm -u+x | head -1)
  [ -n "$EXE" ] || EXE=$(find . -type f -name "*.exe" | head -1)
  [ -n "$EXE" ] || { echo "zip 内未找到网关可执行文件"; exit 1; }
  mkdir -p /opt/easyrdp-gateway/bin
  cp "$EXE" /opt/easyrdp-gateway/bin/easyrdp-gateway && chmod 755 /opt/easyrdp-gateway/bin/easyrdp-gateway
  /opt/easyrdp-gateway/bin/easyrdp-gateway --version 2>/dev/null || true
  # 生产 config：确保 [auth] allow_placeholder_login = false
  CFG=$(ls /opt/easyrdp-gateway/config.toml /etc/easyrdp-gateway/config.toml 2>/dev/null | head -1)
  echo "CFG=$CFG"
  if [ -n "$CFG" ]; then
    grep -q "allow_placeholder_login" "$CFG" || printf "\n[auth]\nallow_placeholder_login = false\n" >> "$CFG"
    grep -A1 "^\[auth\]" "$CFG" | head -3
  fi
  systemctl restart easyrdp-gateway 2>/dev/null || systemctl restart easyrdp-gateway.service 2>/dev/null || echo "⚠ 网关服务重启方式待人工确认（unit 名见上）"
  sleep 1
  systemctl is-active easyrdp-gateway 2>/dev/null || true
  ss -ltnp 2>/dev/null | grep 8443 || true
'

echo "==> ④ users.db 目录自举准备"
$SSH "mkdir -p /var/lib/easyrdp-web && chown www:www /var/lib/easyrdp-web && chmod 750 /var/lib/easyrdp-web; ls -ld /var/lib/easyrdp-web"

echo "==> ④ 站点资产就位核对（release.sh 已上传 gw-admin.html 与 api/gateway/issue.php）"
$SSH "ls -l $SITE_DIR/gw-admin.html $SITE_DIR/api/gateway/issue.php 2>&1; chown www:www $SITE_DIR/gw-admin.html $SITE_DIR/api/gateway/issue.php 2>/dev/null; ls -l $SITE_DIR/api/gateway/keys.php"

echo "==> ③ nginx 热修复层核对"
$SSH 'NGX=$(nginx -t 2>&1); echo "$NGX"
  grep -rn "api/auth/login\|/admin\|api/resources\|/tunnel" /www/server/panel/vhost/nginx/ /etc/nginx/ 2>/dev/null | grep -i easyrdp | head -20 || true
  nginx -s reload 2>/dev/null && echo "nginx reloaded" || systemctl reload nginx && echo "nginx reloaded(systemd)"'

echo "==> ⑤ 线上回读验证"
echo "-- gw-admin.html 期望 200:"; curl -s -o /dev/null -w "%{http_code}\n" https://easyrdp.cn/gw-admin.html
echo "-- issue.php 空体期望 JSON(400 而非 403 目录页):"; curl -s -X POST https://easyrdp.cn/api/gateway/issue.php -H 'Content-Type: application/json' -d '{}' | head -c 200; echo
echo "-- 公网占位登录期望 403/401（闸门生效）:"; curl -s -o /dev/null -w "%{http_code}\n" -X POST https://easyrdp.cn/api/auth/login -H 'Content-Type: application/json' -d '{"oidc_code":"probe:admin"}'
echo "-- /admin/audit 公网期望 404/403:"; curl -s -o /dev/null -w "%{http_code}\n" https://easyrdp.cn/admin/audit
echo "-- /api/resources 期望 200/405（放行清单内）:"; curl -s -o /dev/null -w "%{http_code}\n" https://easyrdp.cn/api/resources
