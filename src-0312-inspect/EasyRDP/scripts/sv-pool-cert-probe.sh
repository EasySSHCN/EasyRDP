#!/usr/bin/env bash
set -e
FK=/tmp/kf; EK=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$FK"; chmod 600 "$FK"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"
echo "== sv: easyssh pool conf =="
ssh -i "$FK" -p 53422 root@sv.trae.show 'cat /etc/php-fpm.d/easyssh.conf'
echo "== 198: 证书路径候选 =="
ssh -i "$EK" -p 22 root@198.2.231.95 '
  ls /root/.acme.sh/ 2>/dev/null | head -10
  ls /www/server/panel/vhost/cert/ 2>/dev/null
  nginx -T 2>/dev/null | grep -E "ssl_certificate |server_name" | grep -i easyrdp | head -5
'
rm -f "$FK" "$EK"
