#!/usr/bin/env bash
# dr-pull-server-v2.sh —— 重新拉取服务器快照（v2：服务器端 gzip -1 提速 + 排除缓存类）
set -e
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
SV=$ROOT/secrets-vault
dec() { openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key"; }
SITE_HOST=$(dec | sed -n 's/^host: \([^ ]*\).*/\1/p')
SITE_PORT=$(dec | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
SITE_DIR=$(dec | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
DR=$ROOT/dr-build/remote-server
mkdir -p "$DR"
KEY=/tmp/kpull
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"; chmod 600 "$KEY"

echo "== 快照1：webroot 全量（gzip -1 提速）=="
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" \
  "rm -f /tmp/wwwroot-full.tar.gz; tar -cf - -C /www/wwwroot easyrdp.cn --exclude='easyrdp.cn/downloads/SHA256SUMS.txt.bak' | gzip -1 > /tmp/wwwroot-full.tar.gz && ls -lh /tmp/wwwroot-full.tar.gz"
scp -q -i "$KEY" -P "${SITE_PORT:-22}" "root@$SITE_HOST:/tmp/wwwroot-full.tar.gz" "$DR/wwwroot-full.tar.gz"
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" "rm -f /tmp/wwwroot-full.tar.gz"

echo "== 快照2：keys.php =="
scp -q -i "$KEY" -P "${SITE_PORT:-22}" "root@$SITE_HOST:$SITE_DIR/api/feedback/keys.php" "$DR/keys.php"

echo "== 快照3：反馈数据 =="
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" "tar -czf /tmp/easyrdp-data.tar.gz -C /www/wwwroot easyrdp-data 2>/dev/null || true; ls -l /tmp/easyrdp-data.tar.gz 2>/dev/null"
scp -q -i "$KEY" -P "${SITE_PORT:-22}" "root@$SITE_HOST:/tmp/easyrdp-data.tar.gz" "$DR/easyrdp-data.tar.gz" 2>/dev/null || echo "（无数据目录）"
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" "rm -f /tmp/easyrdp-data.tar.gz"

echo "== 快照4：系统配置样本（nginx vhost + crontab + php-fpm）=="
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" "tar -czf /tmp/sys-config.tar.gz etc/nginx etc/php-fpm.d etc/cron.d 2>/dev/null; ls -l /tmp/sys-config.tar.gz"
scp -q -i "$KEY" -P "${SITE_PORT:-22}" "root@$SITE_HOST:/tmp/sys-config.tar.gz" "$DR/sys-config-198.tar.gz" 2>/dev/null || true
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" "rm -f /tmp/sys-config.tar.gz"

rm -f "$KEY"
ls -lh "$DR"
echo "✅ 服务器快照拉取完成"
