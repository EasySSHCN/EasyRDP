#!/usr/bin/env bash
set -e
FK=/tmp/kf
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$FK"; chmod 600 "$FK"
ssh -i "$FK" -p 53422 root@sv.trae.show '
  curl -sk -o /dev/null -w "download.html HTTPS %{http_code}\n" -H "Host: easyrdp.cn" https://127.0.0.1/download.html
  curl -sk -H "Host: easyrdp.cn" https://127.0.0.1/api/update/latest.json | head -c 80; echo
  curl -sk -o /dev/null -w "admin.html HTTPS %{http_code}\n" -H "Host: easyrdp.cn" https://127.0.0.1/admin.html
  curl -sk -o /dev/null -w "stats visit.php HTTPS %{http_code}\n" -H "Host: easyrdp.cn" "https://127.0.0.1/api/stats/visit.php?action=ping" 2>/dev/null || true
'
rm -f "$FK"
