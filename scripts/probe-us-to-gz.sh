#!/usr/bin/env bash
set -e
KE=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
echo "== 从 us 探测 gz:22 =="
ssh -i "$KE" root@us.trae.show 'timeout 15 ssh -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new -o BatchMode=yes root@gz.trae.show "echo US-TO-GZ-OK; hostname" 2>&1 | tail -2; timeout 8 bash -c "echo > /dev/tcp/101.96.228.27/22 && echo TCP22-OPEN || echo TCP22-CLOSED" 2>&1 | tail -1'
rm -f "$KE"
