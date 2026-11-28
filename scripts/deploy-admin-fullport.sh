#!/usr/bin/env bash
# deploy-admin-fullport.sh —— 部署 4 标签后台全量复刻版到 198 主机
set -e
KEY=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"; chmod 600 "$KEY"
SITE=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-site
R="root@198.2.231.95"

echo "== 上传 =="
scp -q -i "$KEY" -P 22 "$SITE/admin.html" "$R:/www/wwwroot/easyrdp.cn/admin.html"
ssh -i "$KEY" -p 22 "$R" 'mkdir -p /www/wwwroot/easyrdp.cn/api/stats /www/wwwroot/easyrdp.cn/assets/data /www/wwwroot/easyrdp.cn/tools'
scp -q -i "$KEY" -P 22 "$SITE/api/stats/common.php" "$SITE/api/stats/admin.php" "$SITE/api/stats/visit.php" "$R:/www/wwwroot/easyrdp.cn/api/stats/"
scp -q -i "$KEY" -P 22 "$SITE/assets/data/site-updates.json" "$R:/www/wwwroot/easyrdp.cn/assets/data/site-updates.json"
scp -q -i "$KEY" -P 22 "$SITE/tools/gen-site-updates.js" "$R:/www/wwwroot/easyrdp.cn/tools/gen-site-updates.js" 2>/dev/null || \
  ssh -i "$KEY" -p 22 "$R" 'mkdir -p /www/wwwroot/easyrdp.cn/tools && scp -q -i /dev/null root@127.0.0.1:/dev/null 2>/dev/null; true'

echo "== PHP 语法（服务器端）=="
ssh -i "$KEY" -p 22 "$R" '
  for f in admin common visit; do php -l /www/wwwroot/easyrdp.cn/api/stats/$f.php | tail -1; done
  php -l /www/wwwroot/easyrdp.cn/api/update/check.php | tail -1
  chown www:www /www/wwwroot/easyrdp.cn/admin.html /www/wwwroot/easyrdp.cn/api/stats/*.php /www/wwwroot/easyrdp.cn/assets/data/site-updates.json
'

echo "== 冒烟 =="
curl -s -o /dev/null -w "admin.html %{http_code}\n" "https://easyrdp.cn/admin.html?nocache=7"
curl -s -o /dev/null -w "site-updates.json %{http_code}\n" "https://easyrdp.cn/assets/data/site-updates.json"
curl -s -X POST "https://easyrdp.cn/api/stats/admin.php" -H "Content-Type: application/json" -d '{"action":"summary","user":"x","token":"bad"}' -o /dev/null -w "stats/admin 鉴权拦截 %{http_code}\n"

echo "== 触发 EasyRDP→sv 同步（立即跟进备机）=="
ssh -i "$KEY" -p 22 "$R" '/root/easyrdp-sync-to-sv.sh && tail -2 /var/log/easyrdp-sync.log'

rm -f "$KEY"
echo "✅ 部署完成"
