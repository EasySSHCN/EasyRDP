#!/usr/bin/env bash
K=/tmp/kgz
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$K"; chmod 600 "$K"
timeout 30 ssh -i "$K" root@gz.trae.show 'rsync --partial --append-verify --timeout=30 --info=progress2 -e "ssh -o StrictHostKeyChecking=accept-new" root@us.trae.show:/root/dr-remote-server-20261122.zip /root/dr-remote-server-20261122.zip' 2>&1 | tail -4
echo "RC=$? (124=截断,只看速率)"
rm -f "$K"
