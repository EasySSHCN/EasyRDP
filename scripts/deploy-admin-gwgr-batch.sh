#!/usr/bin/env bash
set -e
KEY=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"
chmod 600 "$KEY"
SITE=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-site
R="root@198.2.231.95"

echo "== 上传 admin.html =="
scp -q -i "$KEY" -P 22 "$SITE/admin.html" "$R:/www/wwwroot/easyrdp.cn/admin.html"

echo "== 权限 + 线上标记核对 =="
ssh -i "$KEY" -p 22 "$R" "chown www:www /www/wwwroot/easyrdp.cn/admin.html && grep -c 'GWGR-BATCH-20261121' /www/wwwroot/easyrdp.cn/admin.html && grep -c 'gwgr-batch-del' /www/wwwroot/easyrdp.cn/admin.html"

echo "== 冒烟 =="
curl -s -o /dev/null -w "admin.html %{http_code}\n" "https://easyrdp.cn/admin.html?nocache=batch1"

echo "== 触发备机同步 =="
ssh -i "$KEY" -p 22 "$R" "/root/easyrdp-sync-to-sv.sh && tail -2 /var/log/easyrdp-sync.log"

rm -f "$KEY"
echo "DEPLOY_DONE"
