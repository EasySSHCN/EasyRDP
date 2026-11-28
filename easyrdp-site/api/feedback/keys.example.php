<?php exit(0); ?>
{
  "_说明": "keys.example.php —— 模板文件。真实 keys.php 只存在于服务器 easyrdp.cn:/www/wwwroot/easyrdp.cn/api/feedback/keys.php，绝不入库/入发布包（脱敏门禁覆盖）。master 为 32 字节 base64；token_secret 为 64 位 hex；admin_hash 为 password_hash() 产物；mail 为 HTTP 邮件 API 网关配置。",
  "master": "REPLACE_WITH_BASE64_OF_32_RANDOM_BYTES",
  "admin_user": "admin",
  "admin_hash": "$2y$10$REPLACE_WITH_PASSWORD_HASH",
  "token_secret": "REPLACE_WITH_64_HEX_CHARS",
  "trusted_proxies": "127.0.0.1,::1",
  "admin_email": "",
  "mail": { "url": "", "key": "" }
}
