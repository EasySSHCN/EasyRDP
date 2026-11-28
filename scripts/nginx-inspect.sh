#!/usr/bin/env bash
set -u
KEY=/tmp/id_ed
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY" && chmod 600 "$KEY"
trap 'rm -f "$KEY"' EXIT
ssh -q -i "$KEY" -p 22 root@easyrdp.cn 'cat /www/server/panel/vhost/nginx/extension/easyrdp.cn/gateway-proxy.conf'
