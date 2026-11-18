#!/usr/bin/env bash
set -e
scp -q -i "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" -P 22 root@198.2.231.95:/www/wwwroot/easyrdp.cn/api/feedback/admin.php /tmp/rdp-admin.php
mkdir -p /mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/dr-build/remote-server/tmp-debug
cp /tmp/rdp-admin.php /mnt/b/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasyRDP/dr-build/remote-server/tmp-debug/
rm -f /tmp/rdp-admin.php
echo OK
