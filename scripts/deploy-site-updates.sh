#!/usr/bin/env bash
# deploy-site-updates.sh —— 部署官网更新记录体系（gen 脚本/JSON/文档说明）到 198 并同步 sv
set -e
KEY=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"; chmod 600 "$KEY"
SITE=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-site
R="root@198.2.231.95"
ssh -i "$KEY" -p 22 "$R" 'mkdir -p /www/wwwroot/easyrdp.cn/tools /www/wwwroot/easyrdp.cn/assets/data'
scp -q -i "$KEY" -P 22 "$SITE/tools/gen-site-updates.js" "$R:/www/wwwroot/easyrdp.cn/tools/gen-site-updates.js"
scp -q -i "$KEY" -P 22 "$SITE/assets/data/site-updates.json" "$R:/www/wwwroot/easyrdp.cn/assets/data/site-updates.json"
ssh -i "$KEY" -p 22 "$R" '
  chown www:www /www/wwwroot/easyrdp.cn/tools/gen-site-updates.js /www/wwwroot/easyrdp.cn/assets/data/site-updates.json
  curl -sk -H "Host: easyrdp.cn" https://127.0.0.1/assets/data/site-updates.json | head -c 200; echo
'
echo "== 触发 EasyRDP→sv 同步 =="
ssh -i "$KEY" -p 22 "$R" '/root/easyrdp-sync-to-sv.sh && tail -1 /var/log/easyrdp-sync.log'
rm -f "$KEY"
echo "✅ 官网更新记录体系部署完成"
