<?php exit(0); ?>
{
  "_说明": "keys.example.php —— update 接口模板文件（R-20261116-005）。真实 keys.php 只存在于服务器 easyrdp.cn:/www/wwwroot/easyrdp.cn/api/update/keys.php，绝不入库/入发布包。master 为 32 字节 base64，来源 = secrets-vault/ota-keys.enc 中的 update_master_b64（由 scripts/ota-keygen.ps1 生成，人工注入）。派生规则与客户端约定：encKey=HMAC-SHA256(master,\"update-enc\")，macKey=HMAC-SHA256(master,\"update-mac\")——与 feedback 族的 'enc'/'mac' 不同 salt，防跨接口密钥复用。",
  "master": "REPLACE_WITH_BASE64_OF_32_RANDOM_BYTES"
}
