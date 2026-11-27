#!/usr/bin/env bash
# la-read-sync.sh —— 读取 la 上现有 EasySSH→sv 同步脚本与调用点（只读）
set -e
KEY=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"; chmod 600 "$KEY"
ssh -i "$KEY" -p 4685 root@la.trae.show '
  echo "== /root/easyssh-sync-to-sv.sh =="; cat /root/easyssh-sync-to-sv.sh
  echo "== 调用点 =="; grep -rn "easyssh-sync-to-sv" /root /etc/cron* /var/spool/cron 2>/dev/null | grep -v Binary | head -5
  echo "== deploy.sh 尾部 =="; tail -20 /opt/dsh/workspace/easyssh-site/tools/deploy.sh 2>/dev/null
'
rm -f "$KEY"
