#!/usr/bin/env bash
set -e
KE=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
ssh -i "$KE" root@us.trae.show "printf '\nssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIG+lXbk1wpE1T+aYGDAuBUa8noIMzmVMGQG5w3jQ5n1c root@iv-yelqu1y3nkf8b8x6uff2\n' >> /root/.ssh/authorized_keys; tail -1 /root/.ssh/authorized_keys | cut -c1-50; echo INJECT-OK"
rm -f "$KE"
