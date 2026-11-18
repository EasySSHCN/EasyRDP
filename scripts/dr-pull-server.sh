#!/usr/bin/env bash
# dr-pull-server.sh —— 从官网服务器拉取远程角色全部数据（keys.php/反馈数据/downloads 全量）
set -e
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
SV=$ROOT/secrets-vault
dec() { openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key"; }
SITE_HOST=$(dec | sed -n 's/^host: \([^ ]*\).*/\1/p')
SITE_PORT=$(dec | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
SITE_DIR=$(dec | sed -n 's|^webroot: \([^ ]*\).*|\1|p')
DR=$ROOT/dr-build/remote-server
mkdir -p "$DR"
KEY=/tmp/id_ed25519_easyssh
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"
chmod 600 "$KEY"
echo "== 远端目录占用调查 =="
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" "
  du -sh $SITE_DIR/downloads $SITE_DIR/api 2>/dev/null
  ls $SITE_DIR | head -20
  echo '--- data 目录 ---'
  ls -d /www/wwwroot/easyrdp-data 2>/dev/null && du -sh /www/wwwroot/easyrdp-data
  echo '--- keys.php ---'
  ls -l $SITE_DIR/api/feedback/keys.php
  echo '--- php 版本 ---'
  php -v | head -1
"
echo "== 拉取 keys.php =="
scp -q -i "$KEY" -P "${SITE_PORT:-22}" "root@$SITE_HOST:$SITE_DIR/api/feedback/keys.php" "$DR/keys.php"
echo "== 拉取反馈数据目录 =="
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" "tar -czf /tmp/easyrdp-data.tar.gz -C /www/wwwroot easyrdp-data 2>/dev/null || tar -czf /tmp/easyrdp-data.tar.gz -C /www/wwwroot easyrdp-data/ 2>/dev/null || echo NODATA"
scp -q -i "$KEY" -P "${SITE_PORT:-22}" "root@$SITE_HOST:/tmp/easyrdp-data.tar.gz" "$DR/easyrdp-data.tar.gz" || echo "（无 easyrdp-data 目录）"
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" "rm -f /tmp/easyrdp-data.tar.gz"
echo "== 全量拉取 webroot（webroot + downloads 全量）=="
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" "tar -czf /tmp/wwwroot-full.tar.gz -C /www/wwwroot easyrdp.cn"
scp -q -i "$KEY" -P "${SITE_PORT:-22}" "root@$SITE_HOST:/tmp/wwwroot-full.tar.gz" "$DR/wwwroot-full.tar.gz"
ssh -i "$KEY" -p "${SITE_PORT:-22}" "root@$SITE_HOST" "rm -f /tmp/wwwroot-full.tar.gz"
rm -f "$KEY"
ls -l "$DR"
du -sh "$DR"
echo "✅ 远程角色数据拉取完成"
