#!/usr/bin/env bash
# fix-mail-channel.sh —— 从 la 读取邮件网关配置，写入 198 keys.php（原子更新+验证）
set -e
EK=/tmp/kek
cp "/mnt/p/Program Files (x86)/EasySSH/keys/id_ed25519_easyssh" "$EK"; chmod 600 "$EK"

echo "== [1] 读取 la 的邮件网关配置 =="
ssh -i "$EK" -p 4685 root@la.trae.show '
  php -r "
    \$t = file_get_contents(\"/www/wwwroot/easyssh.cn/api/feedback/keys.php\");
    \$j = json_decode(preg_replace(\"/^<\?php[^?]*\?>\s*/\", \"\", \$t), true);
    \$m = \$j[\"mail\"] ?? [];
    echo \"MAIL_URL=\" . base64_encode((string)(\$m[\"url\"] ?? \"\")) . \"\n\";
    echo \"MAIL_KEY=\" . base64_encode((string)(\$m[\"key\"] ?? \"\")) . \"\n\";
    echo \"MAIL_URL_SET=\" . ((\$m[\"url\"] ?? \"\") !== \"\" ? \"yes\" : \"no\") . \"\n\";
  "
' > /tmp/mail-creds.txt
cat /tmp/mail-creds.txt | grep MAIL_URL_SET
MAIL_URL=$(base64 -d <(grep '^MAIL_URL=' /tmp/mail-creds.txt | cut -d= -f2-))
MAIL_KEY=$(base64 -d <(grep '^MAIL_KEY=' /tmp/mail-creds.txt | cut -d= -f2-))
echo "  mail.url 长度: ${#MAIL_URL}  mail.key 长度: ${#MAIL_KEY}"
[ -n "$MAIL_URL" ] || { echo "✗ la 侧邮件配置也为空，终止"; rm -f "$EK" /tmp/mail-creds.txt; exit 1; }

echo "== [2] 写入 198 keys.php（备份→原子更新→验证）=="
ssh -i "$EK" -p 22 root@198.2.231.95 MAIL_URL="$MAIL_URL" MAIL_KEY="$MAIL_KEY" 'bash -s' <<'REMOTE'
set -e
K=/www/wwwroot/easyrdp.cn/api/feedback/keys.php
cp "$K" "$K.bak-mail-$(date +%s)"
export K
php -r '
  $k = file_get_contents(getenv("K"));
  $j = json_decode(preg_replace("/^<\?php[^?]*\?>\s*/", "", $k), true);
  $j["mail"]["url"] = getenv("MAIL_URL");
  $j["mail"]["key"] = getenv("MAIL_KEY");
  if (($j["admin_email"] ?? "") === "") $j["admin_email"] = "CTO@7xm.net";
  $new = "<?php exit(0); ?>\n" . json_encode($j, JSON_UNESCAPED_UNICODE | JSON_PRETTY_PRINT) . "\n";
  file_put_contents(getenv("K"), $new, LOCK_EX);
  echo "keys.php mail 配置已写入\n";
'
php -r '
  $j = json_decode(preg_replace("/^<\?php[^?]*\?>\s*/", "", file_get_contents(getenv("K"))), true);
  echo "mail.url=" . substr((string)($j["mail"]["url"] ?? ""), 0, 30) . "...\n";
  echo "mail.key 长度=" . strlen((string)($j["mail"]["key"] ?? "")) . "\n";
  echo "admin_email=" . ($j["admin_email"] ?? "") . "\n";
  // 发一封测试邮件验证通道
  $ch = curl_init();
  curl_setopt_array($ch, [
    CURLOPT_URL => $j["mail"]["url"],
    CURLOPT_POST => true,
    CURLOPT_RETURNTRANSFER => true,
    CURLOPT_HTTPHEADER => ["Content-Type: application/json", "Authorization: Bearer " . $j["mail"]["key"]],
    CURLOPT_POSTFIELDS => json_encode(["to" => $j["admin_email"], "subject" => "EasyRDP 官网邮件通道配置验证", "text" => "这是一封配置验证邮件，收到即表示邮件通道正常。" ]),
    CURLOPT_TIMEOUT => 20,
  ]);
  $resp = curl_exec($ch);
  echo "测试邮件响应: " . substr((string)$resp, 0, 120) . "\n";
'
REMOTE

rm -f "$EK" /tmp/mail-creds.txt
echo "✅ 邮件通道配置完成（请重试后台绑定邮箱）"
