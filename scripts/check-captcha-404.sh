#!/usr/bin/env bash
set -e
EK=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"
ssh -i "$EK" -p 22 root@198.2.231.95 '
  echo "== api/captcha 目录 =="
  ls -la /www/wwwroot/easyrdp.cn/api/captcha/ 2>/dev/null || echo "目录不存在!"
  echo "== 本机 curl 验证码接口 =="
  curl -s -o /dev/null -w "img.php HTTP %{http_code}\n" "https://127.0.0.1/api/captcha/img.php?t=1" -H "Host: easyrdp.cn" -k
  echo "== easyrdp vhost 是否有拦截规则 =="
  grep -n "captcha\|deny\|404" /etc/nginx/conf.d/*.conf 2>/dev/null | head -5
  echo "== BT 面板主配置里的 easyrdp 段（可能另一份 vhost）=="
  ls /www/server/panel/vhost/nginx/ 2>/dev/null
  grep -rn "easyrdp" /www/server/panel/vhost/nginx/*.conf 2>/dev/null | head -3
  echo "== 最近错误日志 =="
  tail -5 /www/wwwlogs/easyrdp.cn.error.log 2>/dev/null || tail -5 /var/log/nginx/easyrdp.cn.error.log 2>/dev/null
'
rm -f "$EK"
