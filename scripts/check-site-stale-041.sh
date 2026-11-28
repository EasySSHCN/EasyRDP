#!/usr/bin/env bash
for p in index.html download.html changelog.html faq.html features.html security.html docs.html compare.html migrate.html contributing.html license.html manual/index.html manual/git-sync-no-git.html en/index.html en/download.html en/changelog.html en/faq.html; do
  echo "== $p =="
  curl -s "https://easyrdp.cn/$p?nocache=$RANDOM" | grep -o 'v0\.4\.0\|0\.4\.0\|v0\.3\.14\|窗口尺寸记忆\|window-size memory\|site-updates[^"]*' | sort | uniq -c | head -8
done
echo "== api/update/site-updates.json =="
curl -s -o /dev/null -w '%{http_code}\n' "https://easyrdp.cn/api/update/site-updates.json?nocache=$RANDOM"
curl -s "https://easyrdp.cn/api/update/site-updates.json?nocache=$RANDOM" | head -c 400; echo
echo "== main.js badge =="
curl -s "https://easyrdp.cn/assets/js/main.js" | grep -o 'SITE_VERSION = "[^"]*"'
