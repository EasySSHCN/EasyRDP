#!/usr/bin/env bash
set -e
FK=/tmp/kf
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$FK"; chmod 600 "$FK"
ssh -i "$FK" -p 53422 root@sv.trae.show '
  set -e
  command -v semanage >/dev/null || dnf install -y policycoreutils-python-utils >/dev/null 2>&1
  semanage fcontext -a -t httpd_sys_content_t "/www/wwwroot/easyrdp.cn(/.*)?" 2>/dev/null || semanage fcontext -m -t httpd_sys_content_t "/www/wwwroot/easyrdp.cn(/.*)?"
  semanage fcontext -a -t httpd_sys_rw_content_t "/www/wwwroot/easyrdp-data(/.*)?" 2>/dev/null || semanage fcontext -m -t httpd_sys_rw_content_t "/www/wwwroot/easyrdp-data(/.*)?"
  restorecon -R /www/wwwroot/easyrdp.cn /www/wwwroot/easyrdp-data
  ls -Zd /www/wwwroot/easyrdp.cn /www/wwwroot/easyrdp.cn/download.html /www/wwwroot/easyrdp-data
  curl -sk -o /dev/null -w "download.html HTTPS %{http_code}\n" -H "Host: easyrdp.cn" https://127.0.0.1/download.html
  curl -sk -o /dev/null -w "admin.html HTTPS %{http_code}\n" -H "Host: easyrdp.cn" https://127.0.0.1/admin.html
  curl -sk -H "Host: easyrdp.cn" https://127.0.0.1/api/update/latest.json | head -c 60; echo
'
rm -f "$FK"
