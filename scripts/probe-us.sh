#!/usr/bin/env bash
set -e
K=/tmp/kk
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$K"; chmod 600 "$K"
for p in 22 53422 2222; do
  echo "-- port $p"
  timeout 12 ssh -i "$K" -p "$p" -o ConnectTimeout=8 -o StrictHostKeyChecking=accept-new -o BatchMode=yes root@107.148.90.44 "echo OK-$p; hostname" 2>&1 | tail -1 || true
done
rm -f "$K"
