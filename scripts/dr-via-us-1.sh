#!/usr/bin/env bash
# dr-via-us.sh —— sv 包经 us(la.trae.show) 中转到本机
set -e
KEY=/tmp/ksv
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$KEY"; chmod 600 "$KEY"
OPTS="ssh -i $KEY -p 53422 -o ConnectTimeout=15 -o StrictHostKeyChecking=accept-new -o ServerAliveInterval=20 -o ServerAliveCountMax=10"

echo "== [1/3] 探测 us(la.trae.show) 可达性 =="
ssh -i "$KEY" -p 53422 -o ConnectTimeout=15 -o StrictHostKeyChecking=accept-new root@la.trae.show 'echo US-OK; hostname; df -h / | tail -1' || { echo "US-UNREACHABLE"; exit 9; }

echo "== [2/3] sv -> us 中转上传（服务器对服务器，应走机房大带宽）=="
ssh -i "$KEY" -p 53422 root@sv.trae.show 'bash -s' <<REMOTE
set -e
KEY2=/tmp/klk
mkdir -p ~/.ssh
# sv -> la 用 sv 自己的出站密钥；若无则现场生成一次性密钥并注入 la
if [ ! -f /root/.ssh/id_ed25519 ]; then
  ssh-keygen -q -t ed25519 -N "" -f /root/.ssh/id_ed25519
fi
PUB=$(cat /root/.ssh/id_ed25519.pub)
ssh -o StrictHostKeyChecking=accept-new -p 53422 root@la.trae.show "mkdir -p ~/.ssh && chmod 700 ~/.ssh; grep -qF '$PUB' ~/.ssh/authorized_keys 2>/dev/null || echo '$PUB' >> ~/.ssh/authorized_keys; chmod 600 ~/.ssh/authorized_keys"
rsync --partial --append-verify --timeout=120 --info=progress2 -e "ssh -i /root/.ssh/id_ed25519 -p 53422 -o StrictHostKeyChecking=accept-new" /root/dr-remote-server-20261122.zip root@la.trae.show:/root/dr-remote-server-20261122.zip
ssh -p 53422 root@la.trae.show "unzip -t -P 'EasyRDP-DR-20261122' /root/dr-remote-server-20261122.zip >/dev/null && echo US-ZIP-OK && ls -lh /root/dr-remote-server-20261122.zip"
echo SV-TO-US-DONE
REMOTE
echo "== 全部中转步骤完成标记 =="
