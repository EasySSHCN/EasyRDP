#!/usr/bin/env bash
set -e
KEY=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"; chmod 600 "$KEY"
SITE=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-site
scp -q -i "$KEY" -P 22 "$SITE/api/stats/client.php" root@198.2.231.95:/www/wwwroot/easyrdp.cn/api/stats/client.php
scp -q -i "$KEY" -P 22 "$SITE/api/feedback/submit.php" root@198.2.231.95:/www/wwwroot/easyrdp.cn/api/feedback/submit.php
ssh -i "$KEY" -p 22 root@198.2.231.95 '
  chown www:www /www/wwwroot/easyrdp.cn/api/stats/client.php /www/wwwroot/easyrdp.cn/api/feedback/submit.php
  php -l /www/wwwroot/easyrdp.cn/api/stats/client.php | tail -1
  php -l /www/wwwroot/easyrdp.cn/api/feedback/submit.php | tail -1
'
rm -f "$KEY"
echo "✅ 归属地写入部署完成"
