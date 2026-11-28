#!/usr/bin/env bash
set -e
K=/tmp/kk
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$K"; chmod 600 "$K"
timeout 20 ssh -i "$K" -p 22 -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new -o BatchMode=yes root@gz.trae.show "echo GZ-OK; hostname; df -h / | tail -1" 2>&1 | tail -4
rm -f "$K"
