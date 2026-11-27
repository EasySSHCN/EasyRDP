#!/usr/bin/env bash
# dr-pull-rsync.sh —— rsync 增量拉取站点目录（可断点续传，重跑即续）
set -e
ROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP
SV=$ROOT/secrets-vault
dec() { openssl enc -aes-256-cbc -d -pbkdf2 -in "$SV/site-server.enc" -pass file:"$SV/key"; }
SITE_HOST=$(dec | sed -n 's/^host: \([^ ]*\).*/\1/p')
SITE_PORT=$(dec | sed -n 's/^ssh: .*port \([0-9]*\)/\1/p')
KEY=/tmp/kpull
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"; chmod 600 "$KEY"
DR=$ROOT/dr-build/remote-server
mkdir -p "$DR/webroot" "$DR/sys-config"

echo "== rsync 站点目录（-z 压缩传输，可续传）=="
rsync -az --partial --info=progress2 \
  -e "ssh -i $KEY -p ${SITE_PORT:-22}" \
  "root@$SITE_HOST:/www/wwwroot/easyrdp.cn/" "$DR/webroot/"

echo "== keys.php =="
rsync -az -e "ssh -i $KEY -p ${SITE_PORT:-22}" "root@$SITE_HOST:/www/wwwroot/easyrdp.cn/api/feedback/keys.php" "$DR/keys.php"

echo "== 反馈数据 =="
rsync -az --partial -e "ssh -i $KEY -p ${SITE_PORT:-22}" "root@$SITE_HOST:/www/wwwroot/easyrdp-data/" "$DR/easyrdp-data/"

echo "== 系统配置样本 =="
rsync -az -e "ssh -i $KEY -p ${SITE_PORT:-22}" "root@$SITE_HOST:/etc/nginx/" "$DR/sys-config/nginx/" 2>/dev/null || true
rsync -az -e "ssh -i $KEY -p ${SITE_PORT:-22}" "root@$SITE_HOST:/etc/cron.d/" "$DR/sys-config/cron.d/" 2>/dev/null || true

rm -f "$KEY"
du -sh "$DR"/*
echo "✅ rsync 拉取完成"
