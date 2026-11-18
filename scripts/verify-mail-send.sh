#!/usr/bin/env bash
set -e
EK=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"
ssh -i "$EK" -p 22 root@198.2.231.95 '
  cd /www/wwwroot/easyrdp.cn/api/feedback
  php -r "
    require \"common.php\";
    \$r = fb_mail_send(\"CTO@7xm.net\", \"EasyRDP 官网邮件通道配置验证\", \"收到此邮件即表示 EasyRDP 官网邮件通道已配置正常。（管理员邮箱绑定功能验证）\");
    echo \$r === true ? \"fb_mail_send: 发送成功 ✓\" : \"fb_mail_send 失败: \" . print_r(\$r, true);
    echo \"\n\";
  "
'
rm -f "$EK"
