#!/usr/bin/env bash
set -e
K=/tmp/kk
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$K"; chmod 600 "$K"
echo "== 本机 -> la:4685 =="
timeout 15 ssh -i "$K" -p 4685 -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new root@la.trae.show 'echo LOCAL-TO-US-OK; hostname; df -h / | tail -1' || { echo LOCAL-FAIL; }
echo "== sv -> la:4685 =="
ssh -i "$K" -p 53422 -o ConnectTimeout=15 -o StrictHostKeyChecking=accept-new root@sv.trae.show 'bash -s' <<'REMOTE'
timeout 15 ssh -p 4685 -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new root@la.trae.show 'echo SV-TO-US-OK; hostname' 2>&1 | tail -2 || echo SV-FAIL
ls /root/.ssh/
REMOTE
rm -f "$K"
