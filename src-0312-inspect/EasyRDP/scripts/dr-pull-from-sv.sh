#!/usr/bin/env bash
# dr-pull-from-sv.sh —— 从 sv 拉取 EasyRDP 站点数据（sv 与主服务器内容一致，每 10 分钟自动同步）
set -e
KEY=/tmp/kpull
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$KEY"; chmod 600 "$KEY"
DR=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/dr-build/remote-server
mkdir -p "$DR/webroot" "$DR/easyrdp-data" "$DR/sys-config"
OPTS="ssh -i $KEY -p 53422 -o StrictHostKeyChecking=accept-new"

echo "== [1/4] rsync webroot（sv → 开发机，测速起步）=="
time rsync -az --partial --info=progress2 -e "$OPTS" "root@sv.trae.show:/www/wwwroot/easyrdp.cn/" "$DR/webroot/"

echo "== [2/4] keys.php =="
rsync -az -e "$OPTS" "root@sv.trae.show:/www/wwwroot/easyrdp.cn/api/feedback/keys.php" "$DR/keys.php"

echo "== [3/4] 反馈数据 =="
rsync -az --partial -e "$OPTS" "root@sv.trae.show:/www/wwwroot/easyrdp-data/" "$DR/easyrdp-data/"

echo "== [4/4] 系统配置样本 =="
rsync -az -e "$OPTS" "root@sv.trae.show:/etc/nginx/conf.d/" "$DR/sys-config/nginx-conf.d/" 2>/dev/null || true
rsync -az -e "$OPTS" "root@sv.trae.show:/etc/php-fpm.d/" "$DR/sys-config/php-fpm.d/" 2>/dev/null || true
rsync -az -e "$OPTS" "root@la.trae.show:/etc/nginx/" "$DR/sys-config/nginx-la/" 2>/dev/null || true

rm -f "$KEY"
du -sh "$DR"/*
echo "✅ 从 sv 拉取完成"
