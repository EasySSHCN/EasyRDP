#!/usr/bin/env bash
# dr-relay-us2.sh —— sv -> us.trae.show(198.2.231.95) 中转
# 授权:本机把 sv 的公钥注入 us 的 authorized_keys(本机对 sv/us 均有密钥),再由 sv rsync 到 us。
set -e
KE=/tmp/kek
KS=/tmp/ksv
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KE"; chmod 600 "$KE"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$KS"; chmod 600 "$KS"

echo "== [1] 取 sv 公钥(现场生成或已有) =="
PUB=$(ssh -i "$KS" -p 53422 -o StrictHostKeyChecking=accept-new root@sv.trae.show 'if [ ! -f /root/.ssh/id_ed25519.pub ]; then ssh-keygen -q -t ed25519 -N "" -f /root/.ssh/id_ed25519; fi; cat /root/.ssh/id_ed25519.pub')
echo "sv pub: ${PUB:0:40}..."

echo "== [2] 注入 us authorized_keys =="
ssh -i "$KE" -o StrictHostKeyChecking=accept-new root@us.trae.show "mkdir -p ~/.ssh && chmod 700 ~/.ssh; grep -qF '$PUB' ~/.ssh/authorized_keys 2>/dev/null || echo '$PUB' >> ~/.ssh/authorized_keys; chmod 600 ~/.ssh/authorized_keys; echo INJECT-OK"

echo "== [3] sv -> us rsync 1.2G =="
ssh -i "$KS" -p 53422 -o StrictHostKeyChecking=accept-new -o ServerAliveInterval=20 root@sv.trae.show 'bash -s' <<'REMOTE'
set -e
rsync --partial --append-verify --timeout=120 --info=progress2 -e "ssh -o StrictHostKeyChecking=accept-new" /root/dr-remote-server-20261122.zip root@us.trae.show:/root/dr-remote-server-20261122.zip
ssh -o StrictHostKeyChecking=accept-new root@us.trae.show "unzip -t -P 'EasyRDP-DR-20261122' /root/dr-remote-server-20261122.zip >/dev/null && echo US-ZIP-OK && ls -lh /root/dr-remote-server-20261122.zip"
echo SV-TO-US-DONE
REMOTE
echo "EXIT-RELAY=$?"
rm -f "$KE" "$KS"
