#!/usr/bin/env bash
set -e
K=/tmp/ksv
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$K"; chmod 600 "$K"
echo "== sv 侧:检查/清理指向 la 的残留 rsync =="
ssh -i "$K" -p 53422 root@sv.trae.show 'pgrep -af "rsync.*la\.trae\.show|rsync.*107\.148" || echo NO-SV-RSYNC; pkill -f "rsync.*107\.148\.90\.44" 2>/dev/null; pkill -f "rsync.*la\.trae\.show" 2>/dev/null; echo CLEANED'
echo "== la 侧:删除未传完的残留文件 =="
timeout 20 ssh -i "$K" -p 4685 root@la.trae.show 'ls -lh /root/dr-remote-server-20261122.zip 2>/dev/null; rm -f /root/dr-remote-server-20261122.zip; pgrep -af rsync || echo NO-LA-RSYNC; echo LA-CLEANED'
rm -f "$K"
