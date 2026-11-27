#!/usr/bin/env bash
for key in id_ed25519_easyssh fleet-master-id_rsa; do
  cp "/mnt/p/Program Files (x86)/EasySSH/keys/$key" /tmp/kj; chmod 600 /tmp/kj
  for p in 22 53422 4685 2222; do
    echo "-- $key port $p"
    timeout 15 ssh -i /tmp/kj -p "$p" -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new -o BatchMode=yes root@jdbj.trae.show "echo JDBJ-OK-$p; hostname" 2>&1 | tail -1
  done
done
rm -f /tmp/kj
