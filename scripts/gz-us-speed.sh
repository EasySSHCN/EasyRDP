#!/usr/bin/env bash
set -e
KE=/tmp/kek
KG=/tmp/kgz
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KG"; chmod 600 "$KG"

echo "== [1] gz 生成临时密钥对 =="
PUB=$(timeout 20 ssh -i "$KG" root@gz.trae.show 'if [ ! -f /root/.ssh/id_ed25519.pub ]; then ssh-keygen -q -t ed25519 -N "" -f /root/.ssh/id_ed25519; fi; cat /root/.ssh/id_ed25519.pub')
echo "gz pub: ${PUB:0:30}..."

echo "== [2] 注入 us authorized_keys(独立新行,保换行) =="
ssh -i "$KE" root@us.trae.show "printf '\n%s\n' '$PUB' >> /root/.ssh/authorized_keys; tail -1 /root/.ssh/authorized_keys | cut -c1-40; echo INJECT-OK"

echo "== [3] gz 从 us 拉取测速(全量 1.2G) =="
timeout 25 ssh -i "$KG" root@gz.trae.show 'rsync --partial --append-verify --timeout=60 --info=progress2 -e "ssh -o StrictHostKeyChecking=accept-new" root@us.trae.show:/root/dr-remote-server-20261122.zip /root/dr-remote-server-20261122.zip' 2>&1 | tail -4
echo "RC=$? (124=超时截断,看速率即可)"
rm -f "$KE" "$KG"
