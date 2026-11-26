#!/usr/bin/env bash
set -e
KE=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
ssh -i "$KE" root@us.trae.show 'echo "== drop set 成员 =="; ipset list in_bt_user_drop_ipset | sed -n "/Members/,+15p"; echo "== firewall-cmd =="; firewall-cmd --state; firewall-cmd --zone=public --list-all 2>/dev/null | head -15; echo "== nft =="; nft list ruleset 2>/dev/null | head -30; echo "== INPUT 全规则 =="; iptables -S | grep -v IN_BT | head -25'
rm -f "$KE"
