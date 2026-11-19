#!/usr/bin/env bash
set -e
EK=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"
ssh -i "$EK" -p 22 root@198.2.231.95 '
  echo "== easyrdp-data 目录 =="
  ls -la /www/wwwroot/easyrdp-data/
  echo "== php-fpm 运行用户 =="
  ps -o user,cmd -C php-fpm 2>/dev/null | head -3 || ps aux | grep [p]hp-fpm | head -3
  echo "== 以 www 身份测试写 admin-sessions.json =="
  sudo -u www touch /www/wwwroot/easyrdp-data/admin-sessions.json 2>&1 && echo "www 可写 ✓" || echo "www 不可写 ✗"
  ls -l /www/wwwroot/easyrdp-data/admin-sessions.json 2>/dev/null
  echo "== sessions 内容 =="
  cat /www/wwwroot/easyrdp-data/admin-sessions.json 2>/dev/null | head -c 400; echo
'
rm -f "$EK"
