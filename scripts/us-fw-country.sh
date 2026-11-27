#!/usr/bin/env bash
set -e
KE=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
ssh -i "$KE" root@us.trae.show 'echo "== IN_BT_Country =="; iptables -L IN_BT_Country -n --line-numbers | head -25; echo "== ipsets =="; ipset list -name; echo "== accept set 成员数 =="; ipset list in_bt_user_accept_ipset 2>/dev/null | grep -A20 "Members" | head -20'
rm -f "$KE"
