#!/usr/bin/env bash
set -e
KE=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
rsync --partial --append-verify --timeout=120 --info=progress2 -e "ssh -i $KE -o StrictHostKeyChecking=accept-new" root@us.trae.show:/root/dr-remote-server-20261122.zip /mnt/b/EasyRDP-DR-20261122/remote-server.zip
echo US-PULL-DONE
rm -f "$KE"
ls -lh /mnt/b/EasyRDP-DR-20261122/remote-server.zip
