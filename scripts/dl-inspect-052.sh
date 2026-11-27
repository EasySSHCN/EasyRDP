#!/usr/bin/env bash
set -u
KEY=/tmp/id_ed
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY" && chmod 600 "$KEY"
trap 'rm -f "$KEY"' EXIT
ssh -q -i "$KEY" -p 22 root@198.2.231.95 'ls -l /www/wwwroot/easyrdp.cn/downloads/ | grep -Ei "0\.5\.2|enc"; echo ---; ls /www/wwwroot/easyrdp.cn/downloads/ | wc -l'
