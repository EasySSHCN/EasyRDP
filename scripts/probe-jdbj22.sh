#!/usr/bin/env bash
K=/tmp/kj
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$K"; chmod 600 "$K"
timeout 25 ssh -i "$K" -p 22 -o ConnectTimeout=15 root@jdbj.trae.show "echo JDBJ-OK; hostname; df -h / | tail -1" 2>&1
rm -f "$K"
