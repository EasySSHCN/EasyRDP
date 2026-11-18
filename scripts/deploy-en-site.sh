#!/usr/bin/env bash
# deploy-en-site.sh —— 部署英文版 en/ 目录 + 全站页面到 198，同步 sv，线上验证
set -e
KEY=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"; chmod 600 "$KEY"
SITE=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-site
R="root@198.2.231.95"

echo "== [1] 上传 en/ 目录（11 页）=="
ssh -i "$KEY" -p 22 "$R" 'mkdir -p /www/wwwroot/easyrdp.cn/en'
scp -q -i "$KEY" -P 22 "$SITE"/en/*.html "$R:/www/wwwroot/easyrdp.cn/en/"

echo "== [2] 上传更新的中文页（nav EN 切换/间距修正）=="
cd "$SITE"
scp -q -i "$KEY" -P 22 *.html "$R:/www/wwwroot/easyrdp.cn/"

echo "== [3] 权限 =="
ssh -i "$KEY" -p 22 "$R" 'chown -R www:www /www/wwwroot/easyrdp.cn/en /www/wwwroot/easyrdp.cn/*.html'

echo "== [4] 同步 sv =="
ssh -i "$KEY" -p 22 "$R" '/root/easyrdp-sync-to-sv.sh && tail -1 /var/log/easyrdp-sync.log'

rm -f "$KEY"

echo "== [5] 线上验证 =="
for p in en/index.html en/download.html en/migrate.html en/compare.html en/changelog.html; do
  curl -s -o /dev/null -w "$p %{http_code}\n" "https://easyrdp.cn/$p?nocache=$RANDOM"
done
ENIX=$(curl -s "https://easyrdp.cn/en/index.html?nocache=$RANDOM")
echo "en/index EN↔中文切换: $(echo "$ENIX" | grep -c 'hreflang') 处 hreflang"
curl -s "https://easyrdp.cn/assets/data/site-updates.json" | head -c 60; echo
