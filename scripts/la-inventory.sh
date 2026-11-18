#!/usr/bin/env bash
# la-inventory.sh —— EasySSH 主服务器（la.trae.show:4685）只读盘点
set -e
KEY=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$KEY"; chmod 600 "$KEY"
ssh -i "$KEY" -p 4685 -o BatchMode=yes -o ConnectTimeout=20 -o StrictHostKeyChecking=accept-new root@la.trae.show '
  echo "== host =="; hostname
  echo "== webroot 候选 =="; ls -d /www/wwwroot/* /opt/dsh/workspace/* 2>/dev/null
  echo "== 官网站点内容 =="; ls /opt/dsh/workspace/easyssh-site 2>/dev/null | head -15
  echo "== crontab =="; crontab -l 2>/dev/null | head -12 || echo "(空)"
  echo "== sv 同步痕迹 =="; grep -rl "sv\.trae\.show\|104\.233" /opt/dsh/workspace/easyssh-site/tools /root 2>/dev/null | grep -v workspace/easyssh-site/api | head -5
  crontab -l 2>/dev/null | grep -i "sv\|sync" || echo "(crontab 无 sv 同步任务)"
'
rm -f "$KEY"
