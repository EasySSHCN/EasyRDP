#!/usr/bin/env bash
# dr-relay-us.sh —— 第 1 段:sv -> la 中转(svpull 密钥,4685)
set -e
K=/tmp/ksv
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$K"; chmod 600 "$K"
ssh -i "$K" -p 53422 -o ConnectTimeout=15 -o StrictHostKeyChecking=accept-new -o ServerAliveInterval=20 root@sv.trae.show 'bash -s' <<'REMOTE'
set -e
rsync --partial --append-verify --timeout=120 --info=progress2 -e "ssh -i /root/.ssh/id_ed25519_svpull -p 4685 -o StrictHostKeyChecking=accept-new" /root/dr-remote-server-20261122.zip root@la.trae.show:/root/dr-remote-server-20261122.zip
ssh -i /root/.ssh/id_ed25519_svpull -p 4685 -o StrictHostKeyChecking=accept-new root@la.trae.show "unzip -t -P 'EasyRDP-DR-20261122' /root/dr-remote-server-20261122.zip >/dev/null && echo US-ZIP-OK && ls -lh /root/dr-remote-server-20261122.zip"
echo SV-TO-US-DONE
REMOTE
echo "EXIT-RELAY1=$?"
rm -f "$K"
