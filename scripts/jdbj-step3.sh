#!/usr/bin/env bash
K=/tmp/kj
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$K"; chmod 600 "$K"
timeout 30 ssh -i "$K" root@jdbj.trae.show 'rsync --partial --append-verify --timeout=30 --info=progress2 -e "ssh -o StrictHostKeyChecking=accept-new" root@us.trae.show:/root/dr-remote-server-20261122.zip /root/dr-remote-server-20261122.zip' 2>&1 | tail -4
rm -f "$K"
