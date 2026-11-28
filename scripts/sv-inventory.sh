#!/usr/bin/env bash
# sv-inventory.sh —— 盘点 sv 与两台主服务器现状（只读）
set -e
FK=/tmp/kf; EK=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$FK"; chmod 600 "$FK"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"

echo "======== [sv 104.233.200.122:53422] ========"
ssh -i "$FK" -p 53422 root@sv.trae.show '
  echo "-- /www/wwwroot 布局 --"; ls -la /www/wwwroot/ 2>/dev/null | head -12
  echo "-- easyssh.cn 内容 --"; ls /www/wwwroot/easyssh.cn 2>/dev/null | head -15
  echo "-- nginx vhosts --"; ls /etc/nginx/conf.d/ 2>/dev/null; grep -l "server_name" /etc/nginx/nginx.conf 2>/dev/null
  echo "-- server_name 清单 --"; grep -rh "server_name" /etc/nginx/ 2>/dev/null | sort -u
  echo "-- php-fpm --"; systemctl list-units "php*" --no-legend 2>/dev/null
  echo "-- authorized_keys --"; cat /root/.ssh/authorized_keys 2>/dev/null | wc -l
  echo "-- crontab --"; crontab -l 2>/dev/null || echo "(空)"
'

echo "======== [EasyRDP 主 198.2.231.95:22] ========"
ssh -i "$EK" -p 22 root@198.2.231.95 '
  echo "-- crontab --"; crontab -l 2>/dev/null || echo "(空)"
  echo "-- authorized_keys 数量 --"; wc -l < /root/.ssh/authorized_keys 2>/dev/null
  echo "-- webroot 体积 --"; du -sh /www/wwwroot/easyrdp.cn /www/wwwroot/easyrdp-data 2>/dev/null
'

echo "======== [EasySSH 主 la.trae.show] ========"
ssh -i "$FK" -o BatchMode=yes -o ConnectTimeout=15 root@la.trae.show '
  echo "-- host --"; hostname
  echo "-- webroot 候选 --"; ls -d /www/wwwroot/* /opt/dsh/workspace/easyssh-site 2>/dev/null
  echo "-- crontab --"; crontab -l 2>/dev/null | head -10 || echo "(空)"
  echo "-- 同步脚本 --"; ls /opt/dsh/workspace/easyssh-site/tools/ 2>/dev/null | head
' || ssh -i "$EK" -o BatchMode=yes root@la.trae.show 'hostname; crontab -l 2>/dev/null | head -10'

rm -f "$FK" "$EK"
echo "✅ 盘点完成"
