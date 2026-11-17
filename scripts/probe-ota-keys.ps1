# probe-ota-keys.ps1 —— 解密 ota-keys.enc 列出键名与公钥，对比 updater.rs 内嵌常量
$ErrorActionPreference = 'Stop'
Set-Location 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP'
$enc = 'secrets-vault\ota-keys.enc'
$key = 'secrets-vault\key'
$json = & wsl bash -c "openssl enc -aes-256-cbc -d -pbkdf2 -in $enc -pass file:$key 2>$null
$keys = $json | ConvertFrom-Json
"keys: " + (($keys.PSObject.Properties.Name) -join ', ')
$pub = $keys.'ota_ed25519_pub_b64'
"pub_b64: $pub"
# 对照 updater.rs 常量
$src = Get-Content 'easyrdp-client\src-tauri\src\updater.rs' -Raw
if ($src -match 'OTA_PUBKEY_B64: &str = "([^"]+)"') {
    $embedded = $Matches[1]
    "embedded: $embedded"
    if ($embedded -eq $pub) { "MATCH: 一致" } else { "MISMATCH: 不一致 ←—— 签名校验失败根因" }
} else { "embedded 常量未找到" }
