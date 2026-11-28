#!/usr/bin/env bash
K=/tmp/kk
for key in "id_ed25519_easyssh:fleet" "fleet-master-id_rsa"; do
  f="${key%%:*}"; tag="${key##*:}"
  cp "/mnt/p/Program Files (x86)/EasySSH/keys/$f" /tmp/kk; chmod 600 /tmp/kk
  echo "-- $tag ($f)"
  timeout 18 ssh -i /tmp/kk -p 22 -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new -o BatchMode=yes root@gz.trae.show "echo GZ-OK; hostname" 2>&1 | tail -2
done
rm -f /tmp/kk
