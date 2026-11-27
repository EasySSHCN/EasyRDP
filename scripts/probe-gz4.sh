#!/usr/bin/env bash
K=/tmp/kk
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$K"; chmod 600 "$K"
timeout 25 ssh -i "$K" -p 22 -o ConnectTimeout=15 -o StrictHostKeyChecking=accept-new root@gz.trae.show "echo GZ-OK; hostname; df -h / | tail -1" 2>&1
rc=$?
echo "RC=$rc"
rm -f "$K"
