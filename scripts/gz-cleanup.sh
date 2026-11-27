#!/usr/bin/env bash
set -e
KE=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
ssh -i "$KE" root@us.trae.show "grep -vF 'iv-yelqu1y3nkf8b8x6uff2' /root/.ssh/authorized_keys > /tmp/ak && mv /tmp/ak /root/.ssh/authorized_keys && chmod 600 /root/.ssh/authorized_keys; echo GZ-KEY-REVOKED; wc -l < /root/.ssh/authorized_keys"
rm -f "$KE"
