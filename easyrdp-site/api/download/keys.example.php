<?php exit(0); ?>
{
  "_说明": "keys.example.php —— 下载签名模板文件（R-20261119-007）。真实 keys.php 只存在于服务器 easyrdp.cn:/www/wwwroot/easyrdp.cn/api/download/keys.php，绝不入库/入发布包（脱敏门禁覆盖，惯例同 api/feedback 与 api/gateway）。dl_sign_key 为 32 字节 base64 或 64 位 hex 的 HMAC-SHA256 签名密钥（token=随机id+HMAC，文件名不进 token）。bind_ip=true 时 token 绑定申请者 /24 网段（IPv6 取前 4 组）；check_referer=true 时校验 Referer/Origin 必须为本站（头缺失放行，兼容 curl/直开）。rate_per_min 为同 IP 每分钟 token 申请上限。keygen：openssl rand -base64 32 或 [Convert]::ToBase64String((1..32 | ForEach-Object { Get-Random -Maximum 256 }))",
  "dl_sign_key": "REPLACE_WITH_BASE64_OR_HEX_OF_AT_LEAST_32_BYTES",
  "bind_ip": true,
  "check_referer": true,
  "rate_per_min": 10
}
