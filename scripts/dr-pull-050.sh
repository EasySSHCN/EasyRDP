#!/usr/bin/env bash
set -e
EK=/tmp/kek2
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$EK"; chmod 600 "$EK"
OPTS="ssh -i $EK -p 53422 -o ConnectTimeout=15 -o ServerAliveInterval=20 -o ServerAliveCountMax=10"
rsync --partial --append-verify --timeout=120 -e "$OPTS" "root@sv.trae.show:/root/dr-remote-server-20261122.zip" /mnt/b/EasyRDP-DR-20261122/remote-server.zip
echo RSYNC-DONE
rm -f "$EK"
ls -lh /mnt/b/EasyRDP-DR-20261122/remote-server.zip
