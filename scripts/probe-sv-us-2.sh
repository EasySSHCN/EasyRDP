#!/usr/bin/env bash
set -e
K=/tmp/ksv
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$K"; chmod 600 "$K"
ssh -i "$K" -p 53422 -o ConnectTimeout=15 -o StrictHostKeyChecking=accept-new root@sv.trae.show 'bash -s' <<'REMOTE'
timeout 15 ssh -i /root/.ssh/id_ed25519_svpull -p 4685 -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new root@la.trae.show 'echo SV-US-OK; hostname; df -h / | tail -1' 2>&1 | tail -3
# 若 svpull 不行,试注入 sv 默认 ed25519(现场生成)到 la authorized_keys
REMOTE
rm -f "$K"
