#!/usr/bin/env bash
# easyrdp-to-sv-setup.sh —— 在 198 主机与 sv 备机间建立 EasyRDP 单向同步（克隆 easyssh-sync v2 模式）
set -e
FK=/tmp/kf; EK=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/fleet-master-id_rsa" "$FK"; chmod 600 "$FK"
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"
SVSSH="ssh -i $FK -p 53422 -o StrictHostKeyChecking=accept-new root@sv.trae.show"
RDPSSH="ssh -i $EK -p 22 -o StrictHostKeyChecking=accept-new root@198.2.231.95"

echo "== [A] 198 主机：生成专用同步密钥 + 同步脚本 + cron =="
$RDPSSH '
  set -e
  [ -f /root/.ssh/id_ed25519_easyrdp_to_sv ] || ssh-keygen -t ed25519 -N "" -f /root/.ssh/id_ed25519_easyrdp_to_sv -C "easyrdp-primary-to-sv"
  cat > /root/easyrdp-sync-to-sv.sh <<'"'"'SYNC'"'"'
#!/bin/bash
# EasyRDP site DR sync: 198 -> sv  v1（克隆 la->sv easyssh-sync v2 模式）
set -u
exec 9>/var/lock/easyrdp-sync.lock
flock -n 9 || exit 0
KEY=/root/.ssh/id_ed25519_easyrdp_to_sv
SV=104.233.200.122
SSHOPTS="-i $KEY -p 53422 -o BatchMode=yes -o ConnectTimeout=15 -o StrictHostKeyChecking=accept-new"

run_sv() { ssh $SSHOPTS "root@$SV" "$@"; }
rsync_sv() { local flags="$1"; shift; rsync $flags -e "ssh $SSHOPTS" "$@"; }

echo "[$(date "+%F %T")] sync start"
rsync_sv "--archive --delete" /www/wwwroot/easyrdp.cn/ "root@$SV:/www/wwwroot/easyrdp.cn/" || { echo "webroot FAIL"; exit 1; }
rsync_sv "--archive --delete" /www/wwwroot/easyrdp-data/ "root@$SV:/www/wwwroot/easyrdp-data/" || { echo "data FAIL"; exit 1; }
rsync_sv "--archive" /www/server/panel/vhost/cert/easyrdp.cn/ "root@$SV:/etc/nginx/ssl-easyrdp/" || echo "cert FAIL(continue)"
run_sv "chown -R root:root /www/wwwroot/easyrdp.cn && chown -R nginx:nginx /www/wwwroot/easyrdp-data && chmod 600 /www/wwwroot/easyrdp.cn/api/feedback/keys.php 2>/dev/null; restorecon -R /www/wwwroot 2>/dev/null; nginx -t 2>&1 | grep -q successful && systemctl reload nginx; echo SV-FIX-OK"
echo "[$(date "+%F %T")] sync done"
SYNC
  chmod 700 /root/easyrdp-sync-to-sv.sh
  cat > /etc/cron.d/easyrdp-sync <<'"'"'CRON'"'"'
# EasyRDP site DR sync: 198 -> sv（单向，每 10 分钟；与 easyssh-sync 错峰）
8,18,28,38,48,58 * * * * root /root/easyrdp-sync-to-sv.sh >> /var/log/easyrdp-sync.log 2>&1
CRON
  chmod 644 /etc/cron.d/easyrdp-sync
  echo "-- 同步公钥（稍后追加到 sv）--"
  cat /root/.ssh/id_ed25519_easyrdp_to_sv.pub
'
PUB=$($RDPSSH 'cat /root/.ssh/id_ed25519_easyrdp_to_sv.pub')

echo "== [B] sv 备机：pool + vhost + 授权同步公钥 =="
$SVSSH "
  set -e
  # php-fpm pool（克隆 easyssh 模式，独立 sock，互不冲突）
  sed -e 's/\[easyssh\]/[easyrdp]/' -e 's|/run/php-fpm/easyssh.sock|/run/php-fpm/easyrdp.sock|' -e 's|easyssh-error.log|easyrdp-error.log|' /etc/php-fpm.d/easyssh.conf > /etc/php-fpm.d/easyrdp.conf
  # 证书目录（占位自签，真实证书由 198 同步覆盖）
  mkdir -p /etc/nginx/ssl-easyrdp
  if [ ! -f /etc/nginx/ssl-easyrdp/fullchain.pem ]; then
    openssl req -x509 -newkey rsa:2048 -keyout /etc/nginx/ssl-easyrdp/privkey.pem -out /etc/nginx/ssl-easyrdp/fullchain.pem -days 3650 -nodes -subj '/CN=easyrdp.cn' 2>/dev/null
  fi
  # nginx vhost（克隆 easyssh.cn.conf，替换域名/根目录/sock/证书路径）
  sed -e 's/easyssh\.cn www\.easyssh\.cn/easyrdp.cn www.easyrdp.cn/' \
      -e 's|root /www/wwwroot/easyssh.cn;|root /www/wwwroot/easyrdp.cn;|' \
      -e 's|/run/php-fpm/easyssh.sock|/run/php-fpm/easyrdp.sock|' \
      -e 's|/etc/nginx/ssl/fullchain.pem|/etc/nginx/ssl-easyrdp/fullchain.pem|' \
      -e 's|/etc/nginx/ssl/privkey.pem|/etc/nginx/ssl-easyrdp/privkey.pem|' \
      -e 's|easyssh\.cn\.log|easyrdp.cn.log|g' \
      -e 's|easyssh\.cn\.error\.log|easyrdp.cn.error.log|' \
      /etc/nginx/conf.d/easyssh.cn.conf > /etc/nginx/conf.d/easyrdp.cn.conf
  mkdir -p /www/wwwroot/easyrdp.cn /www/wwwroot/easyrdp-data
  # 授权 198 同步公钥（幂等）
  mkdir -p /root/.ssh && chmod 700 /root/.ssh
  grep -qF '$PUB' /root/.ssh/authorized_keys 2>/dev/null || echo '$PUB' >> /root/.ssh/authorized_keys
  php-fpm -t 2>&1 | tail -1 && systemctl reload php-fpm
  nginx -t 2>&1 | tail -1 && systemctl reload nginx
  echo SV-SETUP-OK
"

echo "== [C] 首次全量同步（手动触发 198 侧脚本）=="
$RDPSSH '/root/easyrdp-sync-to-sv.sh && tail -2 /var/log/easyrdp-sync.log 2>/dev/null || tail -3 /root/easyrdp-sync-to-sv.sh'

echo "== [D] sv 本机 Host 头冒烟验证 =="
$SVSSH "curl -s -o /dev/null -w 'download.html HTTP %{http_code}\n' -H 'Host: easyrdp.cn' http://127.0.0.1/download.html; curl -s -o /dev/null -w 'check.php HTTP %{http_code}\n' -H 'Host: easyrdp.cn' http://127.0.0.1/api/update/check.php; ls /www/wwwroot/easyrdp.cn | head -8"

rm -f "$FK" "$EK"
echo "✅ EasyRDP → sv 同步体系部署完成（每 10 分钟自动单向同步）"
