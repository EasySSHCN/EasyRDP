#!/usr/bin/env bash
set -e
KE=/tmp/kek
KJ=/tmp/kj
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KJ"; chmod 600 "$KJ"
ssh -i "$KE" root@us.trae.show "grep -qF 'lavm-1n5yqq820y' /root/.ssh/authorized_keys || printf '\nssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIE0Hfw/XXvSsMzVYlloKFR98QJh7KZXlrZSRlZuXZvho root@lavm-1n5yqq820y\n' >> /root/.ssh/authorized_keys; echo INJECT-OK"
timeout 32 ssh -i "$KJ" root@jdbj.trae.show 'rsync --partial --append-verify --timeout=30 --info=progress2 -e "ssh -o StrictHostKeyChecking=accept-new" root@198.2.231.95:/root/dr-remote-server-20261122.zip /root/dr-remote-server-20261122.zip' 2>&1 | tail -3
rm -f "$KE" "$KJ"
