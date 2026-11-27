#!/usr/bin/env bash
set -e
KE=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
ssh -i "$KE" root@us.trae.show 'for c in IN_BT_user_ip IN_BT_ip IN_BT_user_port IN_BT_log; do echo "== $c =="; iptables -L $c -n --line-numbers | head -25; done'
rm -f "$KE"
