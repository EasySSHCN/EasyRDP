#!/usr/bin/env bash
set -e
KEY=/tmp/k_ezrdp
SVKEY='/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh'
SRC=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-site
H=root@198.2.231.95
W=/www/wwwroot/easyrdp.cn
cp "$SVKEY" "$KEY"
chmod 600 "$KEY"
ssh -i "$KEY" -o StrictHostKeyChecking=accept-new "$H" \
  "cp -a $W/gw-admin.html $W/gw-admin.html.bak-toklist && cp -a $W/api/gateway/issue.php $W/api/gateway/issue.php.bak-toklist && echo backup_ok"
scp -q -i "$KEY" "$SRC/gw-admin.html" "$H:$W/gw-admin.html"
scp -q -i "$KEY" "$SRC/api/gateway/issue.php" "$H:$W/api/gateway/issue.php"
ssh -i "$KEY" "$H" \
  "chown www:www $W/gw-admin.html $W/api/gateway/issue.php && chmod 644 $W/gw-admin.html && chmod 644 $W/api/gateway/issue.php && php -l $W/api/gateway/issue.php && grep -c GW-TOKENLIST $W/gw-admin.html $W/api/gateway/issue.php && echo DEPLOY_OK"
rm -f "$KEY"
