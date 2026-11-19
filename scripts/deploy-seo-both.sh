#!/usr/bin/env bash
set -e
echo "[$(date '+%F %T')] SEO 部署：EasyRDP→198 + EasySSH→la"
EK=/tmp/kek; FK=/tmp/kfk
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"
RD=/tmp/krd
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$RD"; chmod 600 "$RD"
SITE=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-site
SSH198=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasySSH/site-server/site

echo "== [1] EasyRDP SEO 页面+robots+sitemap → 198 =="
cd "$SITE"
scp -q -i "$EK" -P 22 *.html robots.txt sitemap.xml "root@198.2.231.95:/www/wwwroot/easyrdp.cn/"
ssh -i "$EK" -p 22 "root@198.2.231.95" 'mkdir -p /www/wwwroot/easyrdp.cn/en && chown -R www:www /www/wwwroot/easyrdp.cn/en'
scp -q -i "$EK" -P 22 en/*.html "root@198.2.231.95:/www/wwwroot/easyrdp.cn/en/"
ssh -i "$EK" -p 22 "root@198.2.231.95" 'chown -R www:www /www/wwwroot/easyrdp.cn/*.html /www/wwwroot/easyrdp.cn/en /www/wwwroot/easyrdp.cn/robots.txt /www/wwwroot/easyrdp.cn/sitemap.xml && /root/easyrdp-sync-to-sv.sh && tail -1 /var/log/easyrdp-sync.log'

echo "== [2] EasySSH SEO 页面+robots+sitemap → la（权威源+webroot）=="
SSHROOT=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasySSH/site-server/site
ssh -i "$RD" -p 4685 root@la.trae.show 'mkdir -p /opt/dsh/workspace/easyssh-site/site/en /www/wwwroot/easyssh.cn/en'
scp -q -i "$RD" -P 4685 "$SSHROOT"/*.html "root@la.trae.show:/opt/dsh/workspace/easyssh-site/site/"
scp -q -i "$RD" -P 4685 "$SSHROOT"/robots.txt "$SSHROOT"/sitemap.xml "root@la.trae.show:/opt/dsh/workspace/easyssh-site/site/"
scp -q -i "$RD" -P 4685 "$SSHROOT"/en/*.html "root@la.trae.show:/opt/dsh/workspace/easyssh-site/site/en/"
ssh -i "$RD" -p 4685 root@la.trae.show '
  cp /opt/dsh/workspace/easyssh-site/site/*.html /www/wwwroot/easyssh.cn/
  cp /opt/dsh/workspace/easyssh-site/site/robots.txt /opt/dsh/workspace/easyssh-site/site/sitemap.xml /www/wwwroot/easyssh.cn/
  cp /opt/dsh/workspace/easyssh-site/site/en/*.html /www/wwwroot/easyssh.cn/en/
  chown -R www:www /www/wwwroot/easyssh.cn/*.html /www/wwwroot/easyssh.cn/robots.txt /www/wwwroot/easyssh.cn/sitemap.xml /www/wwwroot/easyssh.cn/en
'
rm -f "$EK" "$RD"
echo "== [3] 线上验证 =="
for u in "https://easyrdp.cn/robots.txt" "https://easyrdp.cn/sitemap.xml" "https://easyssh.cn/robots.txt" "https://easyssh.cn/sitemap.xml"; do
  curl -s -o /dev/null -w "$u %{http_code}\n" "$u?nocache=$RANDOM"
done
curl -s "https://easyrdp.cn/index.html" | grep -c 'application/ld+json' || true
echo "[$(date '+%F %T')] ✅ SEO 两站部署完成"
