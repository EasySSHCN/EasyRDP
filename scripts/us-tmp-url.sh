#!/usr/bin/env bash
set -e
KE=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
ssh -i "$KE" root@us.trae.show 'cp /root/dr-remote-server-20261122.zip /www/wwwroot/easyrdp.cn/downloads/dr-tmp-20261122.zip && chown www:www /www/wwwroot/easyrdp.cn/downloads/dr-tmp-20261122.zip && ls -lh /www/wwwroot/easyrdp.cn/downloads/dr-tmp-20261122.zip'
rm -f "$KE"
