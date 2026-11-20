#!/usr/bin/env bash
# deploy-pages-and-collapse.sh —— EasyRDP 页面/后台部署+sv 同步；EasySSH changelog 伸缩部署到 la
set -e
EK=/tmp/kek; FK=/tmp/kf; RD=/tmp/krd
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$FK"; chmod 600 "$FK"
SITE=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-site

echo "== [1] EasyRDP：全站页面上传 198 =="
cd "$SITE"
ssh -i "$EK" -p 22 root@198.2.231.95 'mkdir -p /www/wwwroot/easyrdp.cn'
scp -q -i "$EK" -P 22 *.html root@198.2.231.95:/www/wwwroot/easyrdp.cn/
ssh -i "$EK" -p 22 root@198.2.231.95 '
  chown www:www /www/wwwroot/easyrdp.cn/*.html
  for f in /www/wwwroot/easyrdp.cn/*.html; do grep -q "visit-beacon.js" "$f" || sed -i "s|</body>|<script src=\"assets/js/visit-beacon.js\" async></script>\n</body>|" "$f"; done
  php -l /www/wwwroot/easyrdp.cn/api/stats/visit.php | tail -1
  php -l /www/wwwroot/easyrdp.cn/api/stats/common.php | tail -1
'

echo "== [2] 触发 EasyRDP→sv 同步 =="
ssh -i "$EK" -p 22 root@198.2.231.95 '/root/easyrdp-sync-to-sv.sh && tail -1 /var/log/easyrdp-sync.log'

echo "== [3] EasySSH：changelog 伸缩部署到 la（webroot + 权威源）=="
RDKEY=/tmp/krd
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$RDKEY"; chmod 600 "$RDKEY"
ssh -i "$RDKEY" -p 4685 root@la.trae.show 'mkdir -p /opt/dsh/workspace/easyssh-site/site/assets/js /opt/dsh/workspace/easyssh-site/site/assets/css /www/wwwroot/easyssh.cn/assets/js /www/wwwroot/easyssh.cn/assets/css'
scp -q -i "$RDKEY" -P 4685 /mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasySSH/site-server/site/assets/js/site.js "root@la.trae.show:/opt/dsh/workspace/easyssh-site/site/assets/js/site.js"
scp -q -i "$RDKEY" -P 4685 /mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasySSH/site-server/site/assets/css/style.css "root@la.trae.show:/opt/dsh/workspace/easyssh-site/site/assets/css/style.css"
ssh -i "$RDKEY" -p 4685 root@la.trae.show 'cp /opt/dsh/workspace/easyssh-site/site/assets/js/site.js /www/wwwroot/easyssh.cn/assets/js/site.js; cp /opt/dsh/workspace/easyssh-site/site/assets/css/style.css /www/wwwroot/easyssh.cn/assets/css/style.css; chown www:www /www/wwwroot/easyssh.cn/assets/js/site.js /www/wwwroot/easyssh.cn/assets/css/style.css'
rm -f "$RDKEY"

echo "== [4] 线上验证 EasyRDP =="
for p in migrate compare; do curl -s -o /dev/null -w "$p.html %{http_code}\n" "https://easyrdp.cn/$p.html?nocache=$RANDOM"; done
MIG=$(curl -s "https://easyrdp.cn/index.html?nocache=$RANDOM")
echo "首页 nav 含迁移指南: $(echo "$MIG" | grep -c 'migrate.html')"
CMP=$(curl -s "https://easyrdp.cn/compare.html?nocache=$RANDOM")
echo "compare 页对比表: $(echo "$CMP" | grep -c 'cmp-')"
echo "== [5] 线上验证 EasySSH changelog 伸缩（la 源码标记）=="
ssh -i "$RDKEY" -p 4685 root@la.trae.show 'grep -c "CHANGELOG-COLLAPSE" /opt/dsh/workspace/easyssh-site/site/assets/js/site.js /opt/dsh/workspace/easyssh-site/site/assets/css/style.css 2>/dev/null' 2>/dev/null | Out-String
rm -f "$EK" "$FK" "$RDKEY"
echo "✅ 全部部署完成"
