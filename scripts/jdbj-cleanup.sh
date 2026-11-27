#!/usr/bin/env bash
set -e
KE=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
ssh -i "$KE" root@us.trae.show "grep -vF 'lavm-1n5yqq820y' /root/.ssh/authorized_keys > /tmp/ak && mv /tmp/ak /root/.ssh/authorized_keys && chmod 600 /root/.ssh/authorized_keys; echo JDBJ-KEY-REVOKED; wc -l < /root/.ssh/authorized_keys"
rm -f "$KE"
