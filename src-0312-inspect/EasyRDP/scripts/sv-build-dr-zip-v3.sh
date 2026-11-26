#!/usr/bin/env bash
# sv-build-dr-zip-v3.sh —— sv 端重建远程角色密码 zip（v2 脚本内容已自删，此为独立重写）
set -e
KEY=/tmp/ksv
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$KEY"; chmod 600 "$KEY"
DEST=/mnt/b/EasyRDP-DR-20261117
PW='EasyRDP-DR-20261117'
mkdir -p "$DEST"

ssh -i "$KEY" -p 53422 -o StrictHostKeyChecking=accept-new root@sv.trae.show 'bash -s' <<'REMOTE'
set -e
command -v zip >/dev/null 2>&1 || dnf install -y zip >/dev/null
R=/www/wwwroot/easyrdp.cn
P=/root/dr-pkg/remote-server
rm -rf /root/dr-pkg; mkdir -p "$P"

echo "-- [1] webroot 快照 --"
cd "$R"
tar -czf "$P/webroot.tar.gz" --exclude='downloads/SHA256SUMS.txt.bak' .

echo "-- [2] keys.php 机密 --"
cp api/feedback/keys.php "$P/keys.php"

echo "-- [3] 反馈数据 --"
tar -czf "$P/easyrdp-data.tar.gz" -C /www/wwwroot easyrdp-data

echo "-- [4] install.sh --"
cat > "$P/install.sh" <<'INSTALL'
#!/usr/bin/env bash
# EasyRDP 官网服务器一键部署（Debian/Ubuntu，nginx + PHP-FPM）
# 用法：sudo bash install.sh [--webroot /www/wwwroot] [--server-name easyrdp.cn]
set -euo pipefail
WEBROOT='/www/wwwroot'
SERVER_NAME='easyrdp.cn'
PHP_FPM='php8.2-fpm'
PKGDIR="$(cd "$(dirname "$0")" && pwd)"
for a in "$@"; do
  case "$a" in
    --webroot=*) WEBROOT="${a#--webroot=}" ;;
    --server-name=*) SERVER_NAME="${a#--server-name=}" ;;
  esac
done
[ "$(id -u)" -eq 0 ] || { echo "必须以 root 运行"; exit 1; }
echo "== [1/8] 系统包 =="
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y nginx curl ca-certificates unzip rsync php-fpm php-gd php-curl php-mbstring php-xml php-cli
PHP_FPM="$(systemctl list-unit-files 2>/dev/null | grep -oE '^php[0-9.]+-fpm\.service' | head -1 | sed 's/\.service//' || true)"
PHP_FPM="${PHP_FPM:-php8.2-fpm}"
echo "== [2/8] 解压站点全源 =="
mkdir -p "$WEBROOT"
tar -xzf "$PKGDIR/webroot.tar.gz" -C "$WEBROOT"
echo "== [3/8] keys.php（600）=="
install -m 600 -o root -g root "$PKGDIR/keys.php" "$WEBROOT/$SERVER_NAME/api/feedback/keys.php"
echo "== [4/8] 反馈数据 =="
[ -f "$PKGDIR/easyrdp-data.tar.gz" ] && tar -xzf "$PKGDIR/easyrdp-data.tar.gz" -C "$WEBROOT"
mkdir -p "$WEBROOT/easyrdp-data/stats"
chown -R www-data:www-data "$WEBROOT/easyrdp-data"
echo "== [5/8] 权限 =="
chown -R www:www "$WEBROOT/$SERVER_NAME" || chown -R www-data:www-data "$WEBROOT/$SERVER_NAME"
find "$WEBROOT/$SERVER_NAME" -type d -exec chmod 755 {} \;
find "$WEBROOT/$SERVER_NAME" -type f -exec chmod 644 {} \;
chmod 600 "$WEBROOT/$SERVER_NAME/api/feedback/keys.php"
chown -R www-data:www-data "$WEBROOT/$SERVER_NAME/data" 2>/dev/null || true
chmod 777 "$WEBROOT/$SERVER_NAME/data" 2>/dev/null || true
[ -f "$WEBROOT/$SERVER_NAME/data/feedback-db.json" ] && chmod 666 "$WEBROOT/$SERVER_NAME/data/feedback-db.json" || true
echo "== [6/8] nginx =="
PHP_UPSTREAM="unix:/run/php/${PHP_FPM}.sock"
NGX="/etc/nginx/sites-available/$SERVER_NAME.conf"
cat > "$NGX" <<NGINX
server {
    listen 80;
    server_name $SERVER_NAME;
    root $WEBROOT/$SERVER_NAME;
    index index.html index.htm index.php;
    add_header X-Frame-Options "SAMEORIGIN" always;
    add_header X-Content-Type-Options "nosniff" always;
    add_header Referrer-Policy "strict-origin-when-cross-origin" always;
    location ~* \.html\$ { add_header Cache-Control "no-cache, must-revalidate"; }
    location ~ \.php\$ { include snippets/fastcgi-php.conf; fastcgi_pass $PHP_UPSTREAM; }
    location = /api/update/check { include snippets/fastcgi-php.conf; fastcgi_pass $PHP_UPSTREAM; fastcgi_param SCRIPT_FILENAME $WEBROOT/$SERVER_NAME/api/update/check.php; fastcgi_param SCRIPT_NAME /api/update/check.php; }
    location ~ /\.(?!well-known) { deny all; }
    location ~* (\.user\.ini|\.htaccess|\.htpasswd|\.env.*|\.log|\.sql(\.gz)?)\$ { return 404; }
}
NGINX
ln -sf "$NGX" "/etc/nginx/sites-enabled/$SERVER_NAME.conf"
nginx -t && systemctl reload nginx
echo "== [7/8] PHP-FPM =="
systemctl enable --now "$PHP_FPM" 2>/dev/null || systemctl restart "$PHP_FPM"
echo "== [8/8] 防火墙 =="
command -v ufw >/dev/null 2>&1 && ufw status | grep -q active && { ufw allow 80/tcp; ufw allow 443/tcp; } || true
echo "✅ 官网部署完成：$WEBROOT/$SERVER_NAME"
INSTALL

echo "-- [5] SERVER-NOTES.md --"
cat > "$P/SERVER-NOTES.md" <<'NOTES'
# 远程服务器角色说明（easyrdp.cn）

一键部署：sudo bash install.sh（nginx + PHP-FPM + 站点全源 + keys.php 600 + 反馈数据 + no-cache 缓存 + API 路由）

上线核对：
1. 域名解析；curl -I http://localhost/download.html 200
2. php -v（原机 8.2）；php-fpm 运行中
3. curl https://easyrdp.cn/api/update/latest.json 加密信封正常
4. 软件端检查更新、官网反馈提交全流程回归
5. HTTPS：certbot --nginx -d easyrdp.cn
6. 后台登录 https://easyrdp.cn/admin.html（凭据由持有者保管）

注意：keys.php 明文机密（600 root:root）绝不入 git/发布包；
downloads/ 历史产物删除前确认 download.html 历史版本表格链接；
服务器无定时任务，反馈采集/发布巡检在开发机角色。
NOTES

echo "-- [6] 密码 zip --"
cd /root/dr-pkg
rm -f /root/dr-remote-server-20261117.zip
zip -r -q -P 'EasyRDP-DR-20261117' /root/dr-remote-server-20261117.zip remote-server
ls -lh /root/dr-remote-server-20261117.zip
unzip -t /root/dr-remote-server-20261117.zip > /dev/null && echo "ZIP-OK"
rm -f "$P/keys.php"
REMOTE

echo "== [4] 拉回开发机 =="
mkdir -p "$DEST"
scp -q -i "$KEY" -P 53422 "root@sv.trae.show:/root/dr-remote-server-20261117.zip" "$DEST/remote-server.zip"
ssh -i "$KEY" -p 53422 root@sv.trae.show 'ls -lh /root/dr-pkg/remote-server/'
rm -f "$KEY"
ls -lh "$DEST"
echo "✅ 远程角色密码 zip 完成（密码: $PW）"
