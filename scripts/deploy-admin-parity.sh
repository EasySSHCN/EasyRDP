#!/usr/bin/env bash
# deploy-admin-parity.sh —— 部署 4 标签后台对齐版到 198 主机（admin/统计接口/beacon/数据目录）
set -e
EK=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"
SITE=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-site
R="ssh -i $EK -p 22 root@198.2.231.95"

echo "== 上传文件 =="
scp -q -i "$EK" -P 22 "$SITE/admin.html" root@198.2.231.95:/www/wwwroot/easyrdp.cn/admin.html
ssh -i "$EK" -p 22 root@198.2.231.95 'mkdir -p /www/wwwroot/easyrdp.cn/api/stats'
scp -q -i "$EK" -P 22 "$SITE/api/stats/common.php" "$SITE/api/stats/client.php" "$SITE/api/stats/visit.php" "$SITE/api/stats/updates.php" root@198.2.231.95:/www/wwwroot/easyrdp.cn/api/stats/
scp -q -i "$EK" -P 22 "$SITE/api/update/check.php" root@198.2.231.95:/www/wwwroot/easyrdp.cn/api/update/check.php
scp -q -i "$EK" -P 22 "$SITE/assets/js/visit-beacon.js" root@198.2.231.95:/www/wwwroot/easyrdp.cn/assets/js/visit-beacon.js

echo "== 数据目录与权限 =="
ssh -i "$EK" -p 22 root@198.2.231.95 '
  mkdir -p /www/wwwroot/easyrdp-data/stats
  chown -R www:www /www/wwwroot/easyrdp-data/stats
  chmod -R 775 /www/wwwroot/easyrdp-data/stats
  chown www:www /www/wwwroot/easyrdp.cn/admin.html /www/wwwroot/easyrdp.cn/api/update/check.php /www/wwwroot/easyrdp.cn/assets/js/visit-beacon.js
  chown -R www:www /www/wwwroot/easyrdp.cn/api/stats
  php -l /www/wwwroot/easyrdp.cn/api/stats/common.php | tail -1
  php -l /www/wwwroot/easyrdp.cn/api/stats/visit.php | tail -1
  php -l /www/wwwroot/easyrdp.cn/api/stats/client.php | tail -1
  php -l /www/wwwroot/easyrdp.cn/api/stats/updates.php | tail -1
  php -l /www/wwwroot/easyrdp.cn/api/update/check.php | tail -1
'

echo "== 嵌入访客 beacon（9 个页面）=="
ssh -i "$EK" -p 22 root@198.2.231.95 '
  cd /www/wwwroot/easyrdp.cn
  for f in index.html features.html download.html docs.html changelog.html faq.html security.html contributing.html license.html; do
    [ -f "$f" ] || continue
    grep -q "visit-beacon.js" "$f" && continue
    sed -i "s|</body>|<script src=\"assets/js/visit-beacon.js\" async></script>\n</body>|" "$f"
    echo "  beacon → $f"
  done
  chown www:www *.html 2>/dev/null
'

echo "== 冒烟 =="
curl -s -o /dev/null -w "线上 admin.html %{http_code}\n" "https://easyrdp.cn/admin.html?nocache=9"
curl -s "https://easyrdp.cn/api/stats/visit.php" -o /dev/null -w "stats/visit.php GET %{http_code}\n"
rm -f "$EK"
echo "✅ 后台对齐版部署完成"
