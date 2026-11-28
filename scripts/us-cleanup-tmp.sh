#!/usr/bin/env bash
set -e
KE=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
ssh -i "$KE" root@us.trae.show 'rm -f /www/wwwroot/easyrdp.cn/downloads/dr-tmp-20261122.zip /root/dr-remote-server-20261122.zip; echo US-CLEANED; ls /www/wwwroot/easyrdp.cn/downloads/ | grep dr-tmp || echo NO-TMP-LEFT'
rm -f "$KE"
