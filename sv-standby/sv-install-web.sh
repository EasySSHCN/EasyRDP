#!/usr/bin/env bash
# =============================================================================
# sv-install-web.sh — 在 sv 备用服务器上一次性执行的环境安装脚本（幂等）
# 用法: 在 sv 服务器上以 root 运行:  bash sv-install-web.sh
# 前提: 已由 sync-to-sv.sh 同步过 webroot 与 /etc/letsencrypt。
# 作用: nginx + php-fpm(8.x) + 常用扩展 + 两项目独立 vhost + 独立 php-fpm pool
#       + keys.php 权限加固 + HTML no-cache + /data/ 拦截。
# 重复执行安全: 每步都有存在性判断，已存在的配置不覆盖（模板更新请 diff 手动合并）。
# =============================================================================
set -euo pipefail

log() { echo "[sv-install] $(date '+%F %T') $*"; }

# ---------- 0. 参数（默认值与 sv-sync.conf 一致，可环境变量覆盖） ----------
EASYRDP_WEBROOT="${EASYRDP_WEBROOT:-/www/wwwroot/easyrdp.cn}"
EASYSSH_WEBROOT="${EASYSSH_WEBROOT:-/www/wwwroot/easyssh.cn}"
EASYRDP_DATA="${EASYRDP_DATA:-/www/wwwroot/easyrdp-data}"
EASYSSH_DATA="${EASYSSH_DATA:-/www/wwwroot/data}"
PHP_VER="${PHP_VER:-8.1}"   # 无 8.1 时脚本自动探测可用的 8.x

# ---------- 1. 系统包 ----------
log "安装 nginx / php-fpm / 扩展 ..."
if command -v apt-get >/dev/null 2>&1; then
  apt-get update -qq
  apt-get install -y -qq nginx "php${PHP_VER}-fpm" "php${PHP_VER}-cli" \
    "php${PHP_VER}-curl" "php${PHP_VER}-gd" "php${PHP_VER}-mbstring" \
    "php${PHP_VER}-xml" "php${PHP_VER}-zip" "php${PHP_VER}-openssl" \
    "php${PHP_VER}-json" openssl >/dev/null || {
    PHP_VER=$(ls /etc/php 2>/dev/null | grep -E '^8' | sort -V | tail -1)
    log "回退到已安装 PHP: ${PHP_VER}"
  }
elif command -v yum >/dev/null 2>&1; then
  yum install -y -q epel-release >/dev/null 2>&1 || true
  yum install -y -q nginx php-fpm php-cli php-curl php-gd php-mbstring php-xml \
    php-zip php-openssl openssl >/dev/null
  PHP_VER=$(php -r 'echo PHP_MAJOR_VERSION.".".PHP_MINOR_VERSION;' 2>/dev/null || echo "$PHP_VER")
else
  log "[WARN] 未识别的包管理器，请手动安装 nginx + php-fpm 8.x 后重跑"
fi
PHP_FPM_POOL_DIR="/etc/php/${PHP_VER}/fpm/pool.d"
[[ -d "$PHP_FPM_POOL_DIR" ]] || PHP_FPM_POOL_DIR="/etc/php-fpm.d"
log "PHP: ${PHP_VER}, pool 目录: ${PHP_FPM_POOL_DIR}"

# ---------- 2. 目录布局 ----------
log "创建目录布局 ..."
mkdir -p "$EASYRDP_WEBROOT" "$EASYSSH_WEBROOT" "$EASYRDP_DATA" "$EASYSSH_DATA"
mkdir -p /var/log/php-fpm

# ---------- 3. 独立 php-fpm pool（easyrdp / easyssh 隔离） ----------
write_pool() {
  local name="$1" webroot="$2" datadir="$3"
  local f="$PHP_FPM_POOL_DIR/${name}-sv.conf"
  if [[ -f "$f" ]]; then log "pool 已存在，跳过: $f"; return; fi
  cat > "$f" <<EOF
[${name}-sv]
user = www
group = www
listen = /run/php/php${PHP_VER}-fpm-${name}-sv.sock
listen.owner = www
listen.group = www
pm = dynamic
pm.max_children = 10
pm.start_servers = 2
pm.min_spare_servers = 1
pm.max_spare_servers = 4
php_admin_value[error_log] = /var/log/php-fpm/${name}-sv-error.log
php_admin_flag[log_errors] = on
php_admin_value[open_basedir] = ${webroot}:${datadir}:/tmp
EOF
  log "写入 pool: $f"
}
write_pool easyrdp "$EASYRDP_WEBROOT" "$EASYRDP_DATA"
write_pool easyssh "$EASYSSH_WEBROOT" "$EASYSSH_DATA"

# ---------- 4. nginx vhost 模板（独立 server_name，互不冲突） ----------
gen_vhost() {
  local domain="$1" webroot="$2" pool="$3" datadir="$4" outfile="$5"
  if [[ -f "$outfile" ]]; then log "vhost 已存在，跳过: $outfile"; return; fi
  cat > "$outfile" <<EOF
# 自动生成 by sv-install-web.sh — ${domain} (sv 备用)
server {
    listen 80;
    server_name ${domain} www.${domain};
    root ${webroot};
    index index.html index.php;
    charset utf-8;

    # 运行时数据目录禁止公网访问（site-data.json 除外，按需放开）
    location ~ ^/data/ {
        deny all;
    }
    # 备份/临时文件拦截
    location ~ \.(bak|old|swp|tmp)(\.|$) { return 404; }

    # HTML/页面不缓存，保证升级接口版本即时生效
    location ~* \.(html)$ {
        add_header Cache-Control "no-cache, must-revalidate";
    }
    # 静态资源长缓存
    location ~* \.(js|css|png|jpg|jpeg|gif|webp|avif|svg|ico|woff2?)$ {
        expires 30d;
        add_header Cache-Control "public";
    }
    # downloads 目录（历史产物/安装包）
    location /downloads/ {
        autoindex off;
        expires 7d;
    }

    location ~ \.php$ {
        include fastcgi_params;
        fastcgi_pass unix:/run/php/php${PHP_VER}-fpm-${pool}-sv.sock;
        fastcgi_param SCRIPT_FILENAME \$document_root\$fastcgi_script_name;
        fastcgi_read_timeout 120;
        add_header X-Content-Type-Options nosniff always;
        add_header X-Frame-Options SAMEORIGIN always;
        add_header Referrer-Policy strict-origin-when-cross-origin always;
    }

    # HTTPS（证书由 sync-to-sv.sh 从主服务器同步到 /etc/letsencrypt）
    location /.well-known/acme-challenge/ { root ${webroot}; }
}
# === HTTPS server 块占位 ===
# 证书就位后取消注释或由 certbot --nginx 自动生成:
# server {
#     listen 443 ssl http2;
#     server_name ${domain} www.${domain};
#     root ${webroot};
#     index index.html index.php;
#     ssl_certificate     /etc/letsencrypt/live/${domain}/fullchain.pem;
#     ssl_certificate_key /etc/letsencrypt/live/${domain}/privkey.pem;
#     ssl_protocols TLSv1.2 TLSv1.3;
#     add_header Strict-Transport-Security "max-age=31536000" always;
#     add_header Content-Security-Policy "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; connect-src 'self'" always;
#     location ~ ^/data/ { deny all; }
#     location ~ \.(bak|old|swp|tmp)(\.|$) { return 404; }
#     location ~* \.(html)$ { add_header Cache-Control "no-cache, must-revalidate"; }
#     location ~ \.php$ {
#         include fastcgi_params;
#         fastcgi_pass unix:/run/php/php${PHP_VER}-fpm-${pool}-sv.sock;
#         fastcgi_param SCRIPT_FILENAME \$document_root\$fastcgi_script_name;
#         add_header X-Content-Type-Options nosniff always;
#     }
# }
EOF
  log "生成 vhost: $outfile"
}

VHOST_DIR="/etc/nginx/conf.d"
mkdir -p "$VHOST_DIR"
gen_vhost easyrdp.cn "$EASYRDP_WEBROOT" easyrdp "$EASYRDP_DATA" "$VHOST_DIR/easyrdp.cn-sv.conf"
gen_vhost easyssh.cn "$EASYSSH_WEBROOT" easyssh "$EASYSSH_DATA" "$VHOST_DIR/easyssh.cn-sv.conf"

# ---------- 5. keys.php 权限加固（600，root:www） ----------
log "加固 keys.php 权限 ..."
for w in "$EASYRDP_WEBROOT" "$EASYSSH_WEBROOT"; do
  f="$w/api/feedback/keys.php"
  if [[ -f "$f" ]]; then
    chown root:www "$f"; chmod 600 "$f"
    log "  $(printf '%s -> 600 root:www' "$f")"
  else
    log "  [WARN] 未找到 $f（可能尚未 rsync，先跑 sync-to-sv.sh --full）"
  fi
done

# ---------- 6. 属主 + 服务启动 ----------
log "修属主并启动服务 ..."
WWW_GROUP="www"
getent group www >/dev/null 2>&1 || WWW_GROUP="nginx"
chown -R "www:${WWW_GROUP}" "$EASYRDP_WEBROOT" "$EASYSSH_WEBROOT" "$EASYRDP_DATA" "$EASYSSH_DATA" 2>/dev/null || true
chmod 750 "$EASYRDP_DATA" "$EASYSSH_DATA"

php-fpm -t 2>/dev/null || "${PHP_FPM_POOL_DIR%/pool.d}/../fpm/php-fpm" -t || nginx -t
systemctl enable --now php-fpm "php${PHP_VER}-fpm" 2>/dev/null || systemctl restart "php${PHP_VER}-fpm" 2>/dev/null || service php-fpm restart || true
nginx -t && (systemctl enable --now nginx 2>/dev/null || systemctl reload nginx || service nginx reload)

log "完成。自检建议:"
log "  curl -H 'Host: easyrdp.cn'  http://127.0.0.1/   # 应返回 200/首页"
log "  curl -H 'Host: easyssh.cn'  http://127.0.0.1/   # 应返回 200/首页"
log "  curl http://127.0.0.1/api/feedback/keys.php -H 'Host: easyrdp.cn'  # 应为 0 字节/403"
