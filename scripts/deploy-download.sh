#!/usr/bin/env bash
set -e
cp '/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh' /tmp/k7
chmod 600 /tmp/k7
SRC=/mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/easyrdp-site
H=root@198.2.231.95
W=/www/wwwroot/easyrdp.cn
scp -q -i /tmp/k7 "$SRC/download.html" "$H:$W/download.html"
ssh -i /tmp/k7 "$H" "chown www:www $W/download.html && chmod 644 $W/download.html && grep -c 0.5.0 $W/download.html && echo DL_OK"
rm -f /tmp/k7
