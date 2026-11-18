#!/usr/bin/env bash
# ============================================================================
# EasyRDP 远程服务器角色一键安装脚本（easyrdp.cn 官网）
# ----------------------------------------------------------------------------
# 目标系统：Debian 11/12 或 Ubuntu 20.04/22.04/24.04（x86_64）
# 用途    ：在一台全新 Linux 服务器上完整复刻 easyrdp.cn 角色：
#           nginx + PHP-FPM + 站点全源（页面/api/downloads）+ 反馈数据
#           + keys.php 机密 + nginx/php-fpm 系统配置 + 权限/缓存策略
# 用法    ：sudo bash install.sh [--webroot=/www/wwwroot] [--server-name=easyrdp.cn]
# 幂等    ：重复执行安全。
# ============================================================================
set -euo pipefail

WEBROOT='/www/wwwroot'          # 保持宝塔目录布局约定（与原服务器一致）
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
[ -f "$PKGDIR/wwwroot-full.tar.gz" ] || { echo "缺少 wwwroot-full.tar.gz"; exit 1; }

echo "== [1/8] 系统包：nginx / PHP-FPM / 必备扩展 =="
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y nginx curl ca-certificates gnupg lsb-release \
  php-fpm php-gd php-curl php-mbstring php-xml php-cli unzip rsync
# 探测实际 PHP-FPM 服务名
PHP_FPM="$(systemctl list-unit-files 2>/dev/null | grep -oE '^php[0-9.]+-fpm\.service' | head -1 | sed 's/\.service$//' || true)"
PHP_FPM="${PHP_FPM:-php8.2-fpm}"

echo "== [2/8] 部署站点全源（wwwroot/easyrdp.cn → $WEBROOT/easyrdp.cn）=="
mkdir -p "$WEBROOT"
if [ -d "$PKGDIR/wwwroot/easyrdp.cn" ]; then
  mkdir -p "$WEBROOT/easyrdp.cn"
  cp -a "$PKGDIR/wwwroot/easyrdp.cn/." "$WEBROOT/easyrdp.cn/"
elif [ -f "$PKGDIR/wwwroot-full.tar.gz" ]; then
  tar -xzf "$PKGDIR/wwwroot-full.tar.gz" -C "$WEBROOT"
fi
# 包内顶层目录即 easyrdp.cn/
[ -d "$WEBROOT/$SERVER_NAME" ] || { echo "部署后未找到 $WEBROOT/$SERVER_NAME，检查包结构"; exit 1; }

echo "== [3/8] 反馈数据目录（easyrdp-data）=="
if [ -d "$PKGDIR/easyrdp-data" ]; then
  mkdir -p "$WEBROOT/easyrdp-data"
  cp -a "$PKGDIR/easyrdp-data/." "$WEBROOT/easyrdp-data/"
elif [ -f "$PKGDIR/easyrdp-data.tar.gz" ]; then
  tar -xzf "$PKGDIR/easyrdp-data.tar.gz" -C "$WEBROOT"
fi
mkdir -p "$WEBROOT/easyrdp-data"
chown -R www-data:www-data "$WEBROOT/easyrdp-data"

echo "== [4/8] 部署系统配置快照（sys-config：nginx 站点配置 + php-fpm 池）=="
# sys-config/nginx-conf-sv、sys-config/php-fpm-sv 为原服务器（宝塔 sv 环境）配置快照，
# 按标准 Debian/Ubuntu 布局落地；宝塔布局请自行对照调整。
if [ -d "$PKGDIR/sys-config/nginx-conf-sv" ]; then
  mkdir -p /etc/nginx/sites-available /etc/nginx/conf.d
  cp -f "$PKGDIR"/sys-config/nginx-conf-sv/*.conf /etc/nginx/sites-available/ 2>/dev/null || true
fi
if [ -d "$PKGDIR/sys-config/php-fpm-sv" ]; then
  PHP_POOL_DIR="/etc/php/${PHP_FPM#php}/fpm/pool.d"
  mkdir -p "$PHP_POOL_DIR"
  cp -f "$PKGDIR"/sys-config/php-fpm-sv/*.conf "$PHP_POOL_DIR/" 2>/dev/null || true
fi

echo "== [5/8] nginx 站点配置（生成标准布局配置）=="
NGX="/etc/nginx/sites-available/$SERVER_NAME.conf"
PHP_UPSTREAM="unix:/run/php/${PHP_FPM}.sock"
cat > "$NGX" <<NGINX
server {
    listen 80;
    server_name $SERVER_NAME;
    root $WEBROOT/$SERVER_NAME;
    index index.html index.php;

    # HTML no-cache 协商缓存（发布流程 8b 校验依赖：普通 F5 即见新版）
    location ~* \.html\$ {
        add_header Cache-Control "no-cache, must-revalidate";
    }
    location ~* \.(js|css|png|jpg|svg|ico)\$ {
        expires 7d;
        add_header Cache-Control "public";
    }
    location /downloads/ { }

    location ~ \.php\$ {
        include snippets/fastcgi-php.conf;
        fastcgi_pass $PHP_UPSTREAM;
    }
    location ~ /\.(?!well-known) { deny all; }
}
NGINX
ln -sf "$NGX" "/etc/nginx/sites-enabled/$SERVER_NAME.conf"
rm -f /etc/nginx/sites-enabled/default
nginx -t
systemctl reload nginx

echo "== [6/8] 权限：站点归 www-data，keys.php 收紧 600 =="
chown -R www-data:www-data "$WEBROOT/$SERVER_NAME" || true
find "$WEBROOT/$SERVER_NAME" -type d -exec chmod 755 {} \;
find "$WEBROOT/$SERVER_NAME" -type f -exec chmod 644 {} \;
# 包内 wwwroot-full.tar.gz 已内含真实 keys.php（api/feedback/keys.php 与 api/update/keys.php）
chmod 600 "$WEBROOT/$SERVER_NAME/api/feedback/keys.php" 2>/dev/null || true
chmod 600 "$WEBROOT/$SERVER_NAME/api/update/keys.php" 2>/dev/null || true
mkdir -p "$WEBROOT/$SERVER_NAME/downloads"

echo "== [7/8] PHP-FPM 启动 =="
systemctl enable --now "$PHP_FPM" 2>/dev/null || systemctl restart "$PHP_FPM"

echo "== [8/8] 防火墙放行（如启用 ufw）=="
if command -v ufw >/dev/null 2>&1 && ufw status | grep -q 'Status: active'; then
  ufw allow 80/tcp; ufw allow 443/tcp
fi

echo ""
echo "✅ 远程服务器角色安装完成"
echo "   站点根：$WEBROOT/$SERVER_NAME"
echo "   验证  ：curl -I http://localhost/download.html"
echo "          curl http://localhost/api/update/check.php?ver=0.0.0 | head -c 200"
echo ""
echo "【后续可选】"
echo " 1) HTTPS：certbot --nginx -d $SERVER_NAME（需域名已解析到本机；.well-known/acme 已随包就位）"
echo " 2) 修改反馈后台凭据：编辑 $WEBROOT/$SERVER_NAME/api/feedback/keys.php 的"
echo "    admin_user / admin_hash（password_hash 产物）"
echo " 3) 定时任务：官网角色无需定时任务；反馈采集/发布巡检定时任务位于开发机角色。"
