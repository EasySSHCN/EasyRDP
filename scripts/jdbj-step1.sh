#!/usr/bin/env bash
K=/tmp/kj
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$K"; chmod 600 "$K"
timeout 25 ssh -i "$K" root@jdbj.trae.show 'if [ ! -f /root/.ssh/id_ed25519.pub ]; then ssh-keygen -q -t ed25519 -N "" -f /root/.ssh/id_ed25519; fi; cat /root/.ssh/id_ed25519.pub'
echo "RC=$?"
rm -f "$K"
