#!/usr/bin/env bash
set -e
KE=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
ssh -i "$KE" root@us.trae.show 'iptables -L IN_BT -n --line-numbers | head -40; echo ---; iptables -S IN_BT | head -40'
rm -f "$KE"
