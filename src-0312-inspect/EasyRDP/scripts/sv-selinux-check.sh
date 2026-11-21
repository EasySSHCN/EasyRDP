#!/usr/bin/env bash
set -e
FK=/tmp/kf
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$FK"; chmod 600 "$FK"
ssh -i "$FK" -p 53422 root@sv.trae.show '
  echo "== SELinux =="; getenforce
  echo "== 上下文对比 =="
  ls -Zd /www/wwwroot/easyssh.cn /www/wwwroot/easyrdp.cn
  ls -Z /www/wwwroot/easyssh.cn/download.html /www/wwwroot/easyrdp.cn/download.html 2>/dev/null
  ls -Zd /www/wwwroot/easyssh.cn/api /www/wwwroot/easyrdp.cn/api 2>/dev/null
  echo "== 最近 SELinux 拒绝 =="
  ausearch -m avc -ts recent 2>/dev/null | tail -5 || grep -i "denied" /var/log/audit/audit.log 2>/dev/null | tail -3
'
rm -f "$FK"
