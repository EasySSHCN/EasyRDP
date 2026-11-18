#!/usr/bin/env bash
set -e
FK=/tmp/kf
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$FK"; chmod 600 "$FK"
ssh -i "$FK" -p 53422 root@sv.trae.show '
  echo "== easyssh.cn.conf =="; cat /etc/nginx/conf.d/easyssh.cn.conf
  echo "== php-fpm pool 清单 =="; ls /etc/php-fpm.d/ 2>/dev/null
  echo "== www pool 用户 =="; grep -E "^(user|group|listen) " /etc/php-fpm.d/www.conf 2>/dev/null
  echo "== php 模块(相关) =="; php -m 2>/dev/null | grep -iE "gd|curl|mbstring|json|openssl" 
  echo "== SELinux =="; getenforce 2>/dev/null
'
rm -f "$FK"
