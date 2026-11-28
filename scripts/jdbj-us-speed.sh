#!/usr/bin/env bash
set -e
KE=/tmp/kek
KJ=/tmp/kj
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KJ"; chmod 600 "$KJ"
PUB=$(timeout 20 ssh -i "$KJ" root@jdbj.trae.show 'cat /root/.ssh/id_ed25519.pub')
ssh -i "$KE" root@us.trae.show "grep -qF '$PUB' /root/.ssh/authorized_keys || printf '\n%s\n' '$PUB' >> /root/.ssh/authorized_keys; echo INJECT-OK"
timeout 32 ssh -i "$KJ" root@jdbj.trae.show 'rsync --partial --append-verify --timeout=30 --info=progress2 -e "ssh -o StrictHostKeyChecking=accept-new" root@us.trae.show:/root/dr-remote-server-20261122.zip /root/dr-remote-server-20261122.zip' 2>&1 | tail -3
rm -f "$KE" "$KJ"
