#!/usr/bin/env bash
set -e
KE=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
ssh -i "$KE" root@us.trae.show 'bash -s' <<'REMOTE'
systemctl is-active firewalld 2>/dev/null || echo firewalld-inactive
iptables -S INPUT | head -30
echo ---
iptables -L INPUT -n --line-numbers | head -30
REMOTE
rm -f "$KE"
