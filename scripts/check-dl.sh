#!/usr/bin/env bash
set -e
cp '/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh' /tmp/k6
chmod 600 /tmp/k6
ssh -i /tmp/k6 root@198.2.231.95 'ls /www/wwwroot/easyrdp.cn/downloads/ | grep 0.5.0; echo ---; grep 0.5.0 /www/wwwroot/easyrdp.cn/downloads/SHA256SUMS.txt | head -4'
rm -f /tmp/k6
