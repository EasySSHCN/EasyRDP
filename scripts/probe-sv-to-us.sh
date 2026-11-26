#!/usr/bin/env bash
set -e
K=/tmp/ksv
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$K"; chmod 600 "$K"
echo "== sv 侧探测 la（服务器对服务器）=="
ssh -i "$K" -p 53422 -o ConnectTimeout=15 -o StrictHostKeyChecking=accept-new root@sv.trae.show 'bash -s' <<'REMOTE'
for p in 22 53422 2222; do
  echo "-- la:$p"
  timeout 12 ssh -o ConnectTimeout=8 -o StrictHostKeyChecking=accept-new -o BatchMode=yes root@107.148.90.44 -p $p "echo OK-$p; hostname" 2>&1 | tail -1 || true
done
# 兜底：sv 上是否有可直连 la 的历史凭据
ls -la /root/.ssh/ 2>/dev/null | head
REMOTE
rm -f "$K"
