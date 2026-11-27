#!/usr/bin/env bash
KJ=/tmp/kj
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KJ"; chmod 600 "$KJ"
timeout 32 ssh -i "$KJ" root@jdbj.trae.show 'rsync --partial --append-verify --timeout=30 --info=progress2 -e "ssh -o StrictHostKeyChecking=accept-new" root@198.2.231.95:/root/dr-remote-server-20261122.zip /root/dr-remote-server-20261122.zip' 2>&1 | tail -3
rm -f "$KJ"
