#!/usr/bin/env bash
set -e
K=/tmp/ksv
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$K"; chmod 600 "$K"
PUB=$(ssh -i "$K" -p 53422 root@sv.trae.show 'cat /root/.ssh/id_ed25519.pub' 2>/dev/null || echo "")
KEYPART=$(echo "$PUB" | awk '{print $2}')
timeout 20 ssh -i "$K" -p 4685 root@la.trae.show "grep -vF '$KEYPART' /root/.ssh/authorized_keys > /tmp/ak && mv /tmp/ak /root/.ssh/authorized_keys && chmod 600 /root/.ssh/authorized_keys; echo LA-KEY-REVOKED; wc -l /root/.ssh/authorized_keys"
ssh -i "$K" -p 53422 root@sv.trae.show 'rm -f /root/.ssh/id_ed25519 /root/.ssh/id_ed25519.pub; echo SV-TMP-KEY-REMOVED'
rm -f "$K"
