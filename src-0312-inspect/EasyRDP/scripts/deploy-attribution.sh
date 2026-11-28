#!/usr/bin/env bash
# deploy-attribution.sh —— 部署归因链路服务端（ticket.php/client.php/download.html/tickets 目录）到 198
set -e
KEY=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"; chmod 600 "$KEY"
SITE=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace\EasyRDP/easyrdp-site
SITE=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-site
R="root@198.2.231.95"
ssh -i "$KEY" -p 22 "$R" 'mkdir -p /www/wwwroot/easyrdp.cn/api/download /www/wwwroot/easyrdp-data/tickets && chown www:www /www/wwwroot/easyrdp-data/tickets && chmod 775 /www/wwwroot/easyrdp-data/tickets'
scp -q -i "$KEY" -P 22 "$SITE/api/download/ticket.php" "$R:/www/wwwroot/easyrdp.cn/api/download/ticket.php"
scp -q -i "$KEY" -P 22 "$SITE/api/stats/client.php" "$R:/www/wwwroot/easyrdp.cn/api/stats/client.php"
scp -q -i "$KEY" -P 22 "$SITE/download.html" "$R:/www/wwwroot/easyrdp.cn/download.html"
ssh -i "$EK" -p 22 "$R" '
  chown www:www /www/wwwroot/easyrdp.cn/api/download/ticket.php /www/wwwroot/easyrdp.cn/api/stats/client.php /www/wwwroot/easyrdp.cn/download.html
  php -l /www/wwwroot/easyrdp.cn/api/download/ticket.php | tail -1
  php -l /www/wwwroot/easyrdp.cn/api/stats/client.php | tail -1
'
rm -f "$EK"
echo "== 冒烟：签发票据 =="
curl -sk -X POST "https://easyrdp.cn/api/download/ticket.php" -H "Host: easyrdp.cn" -H "Content-Type: application/json" -d '{}' -o /tmp/ticket-resp.json -w "ticket.php HTTP %{http_code}\n"
cat /tmp/ticket-resp.json | head -c 200; echo
echo "== 触发 sv 同步 =="
ssh -i "$EK" -p 22 "$R" '/root/easyrdp-sync-to-sv.sh && tail -1 /var/log/easyrdp-sync.log'
rm -f "$EK"
echo "✅ 归因链路服务端部署完成"
