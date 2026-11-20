# ============================================================================
# ota-keygen.ps1 —— 生成 OTA 签名密钥对与更新主密钥（需求 R-20261116-005）
# ----------------------------------------------------------------------------
# 用法（本地开发机，手动执行一次；发布会话人工决定何时执行/注入）：
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\ota-keygen.ps1
#   powershell ... -File scripts\ota-keygen.ps1 -Force    # 覆盖已存在的 ota-keys.enc
#
# 产物（均写本地 secrets-vault\，不触网、不上传、不动其它现有文件）：
#   secrets-vault\ota-keys.enc —— AES-256-CBC/PBKDF2 加密（pass file: secrets-vault\key，
#       与 site-server.enc / creds.enc 同模式）。明文为三行 key: value：
#         ota_ed25519_priv_b64: <Ed25519 私钥原始 32 字节 base64>
#         ota_ed25519_pub_b64:  <Ed25519 公钥原始 32 字节 base64>
#         update_master_b64:    <update 接口主密钥（32 字节随机）base64>
#   secrets-vault\ota-pub.b64  —— 仅公钥原始 32 字节 base64（不加密，可随客户端分发用于验签）
#
# 密钥用途（详见 docs\方案-在线更新机制.md）：
#   - Ed25519 私钥：ota-package.ps1 对更新清单 latest.json 签名（服务器公钥验签）
#   - update_master：官网 api/update/keys.php 的 master（派生 encKey/macKey 加解信封）
#     派生规则（与客户端约定）：encKey=HMAC-SHA256(master,"update-enc")，
#     macKey=HMAC-SHA256(master,"update-mac")——与 feedback 族的 'enc'/'mac' 不同 salt。
#
# openssl 来源：优先 PATH 上的 openssl.exe；否则回退 WSL bash 调用（路径经 wslpath 转换）。
# 幂等：ota-keys.enc 已存在时默认跳过并提示（防止误覆盖真实密钥）。
# ============================================================================

param([switch]$Force)

$ErrorActionPreference = 'Stop'

$Root  = Split-Path -Parent $PSScriptRoot
$Vault = Join-Path $Root 'secrets-vault'
$EncOut   = Join-Path $Vault 'ota-keys.enc'
$PubOut   = Join-Path $Vault 'ota-pub.b64'
$KeyFile  = Join-Path $Vault 'key'

if (-not (Test-Path $KeyFile)) { throw "缺少加密口令文件: $KeyFile" }

# ---------- 幂等守卫 ----------
if ((Test-Path $EncOut) -and (-not $Force)) {
  Write-Host "==> secrets-vault\ota-keys.enc 已存在，默认跳过（如需重新生成请加 -Force）"
  exit 0
}

# ---------- openssl 解析（原生 PATH 优先，WSL bash 回退） ----------
$script:OsslMode = $null
if (Get-Command openssl.exe -ErrorAction SilentlyContinue) {
  $script:OsslMode = 'native'
} elseif (Get-Command wsl.exe -ErrorAction SilentlyContinue) {
  $script:OsslMode = 'wsl'
} else {
  throw '未找到 openssl（PATH）也无法回退 WSL——请安装其中之一后重试'
}

# WSL 模式下把 Windows 路径转成 /mnt/... 形式；原生模式原样返回
function ConvertTo-OsslPath([string]$Path) {
  if ($script:OsslMode -eq 'native') { return $Path }
  # 修复: bash 会吞掉反斜杠,先把 \ 换成 / 再交 wslpath(wslpath 接受正斜杠 Windows 路径)
  $p = (wsl.exe -- wslpath -a ($Path -replace '\\', '/') 2>$null | Select-Object -First 1)
  if (-not $p) { throw "wslpath 转换失败: $Path" }
  return $p.Trim()
}

# 统一执行 openssl（参数为一整行；文件参数请先经 ConvertTo-OsslPath 转换）
function Invoke-Ossl([string]$ArgLine) {
  $out = $null
  if ($script:OsslMode -eq 'native') {
    $out = cmd /c "openssl $ArgLine 2>&1"
  } else {
    $out = wsl.exe -- bash -c "openssl $ArgLine 2>&1"
  }
  if ($LASTEXITCODE -ne 0) {
    throw "openssl 执行失败（$($script:OsslMode)）: openssl $ArgLine`n$($out -join "`n")"
  }
  return ($out -join "`n")
}

$TmpDir = Join-Path $env:TEMP ("ota-keygen-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $TmpDir | Out-Null
$PrivPem = Join-Path $TmpDir 'ota-ed25519-priv.pem'
$PubPem  = Join-Path $TmpDir 'ota-ed25519-pub.pem'
$Plain   = Join-Path $TmpDir 'ota-keys.plain'

try {
  Write-Host "==> 生成 Ed25519 签名密钥对（openssl，$($script:OsslMode) 模式）"
  Invoke-Ossl "genpkey -algorithm ed25519 -out `"$(ConvertTo-OsslPath $PrivPem)`"" | Out-Null
  Invoke-Ossl "pkey -in `"$(ConvertTo-OsslPath $PrivPem)`" -pubout -out `"$(ConvertTo-OsslPath $PubPem)`"" | Out-Null

  # PEM → 原始 32 字节 base64：PKCS8 私钥 DER 尾 32 字节即种子；SPKI 公钥 DER 尾 32 字节即原始公钥
  function Get-PemRawB64([string]$PemPath) {
    $lines = Get-Content -Path $PemPath | Where-Object { $_ -notmatch '^-----' }
    $der = [Convert]::FromBase64String((($lines -join '')).Trim())
    if ($der.Length -lt 32) { throw "PEM 解码异常: $PemPath" }
    $raw = New-Object byte[] 32
    [Array]::Copy($der, $der.Length - 32, $raw, 0, 32)
    return [Convert]::ToBase64String($raw)
  }
  $PrivB64 = Get-PemRawB64 $PrivPem
  $PubB64  = Get-PemRawB64 $PubPem

  Write-Host "==> 生成 update 主密钥（32 字节随机）"
  $MasterB64 = (Invoke-Ossl 'rand -base64 32').Trim()

  Write-Host "==> 写入加密库 ota-keys.enc（AES-256-CBC/PBKDF2, pass file: secrets-vault\key）"
  $plainText = "ota_ed25519_priv_b64: $PrivB64`nota_ed25519_pub_b64: $PubB64`nupdate_master_b64: $MasterB64`n"
  [IO.File]::WriteAllText($Plain, $plainText, (New-Object Text.UTF8Encoding($false)))
  Invoke-Ossl ("enc -aes-256-cbc -pbkdf2 -salt -in `"" + (ConvertTo-OsslPath $Plain) + "`" -out `"" + (ConvertTo-OsslPath $EncOut) + "`" -pass file:`"" + (ConvertTo-OsslPath $KeyFile) + "`"") | Out-Null

  Write-Host "==> 写入公钥 ota-pub.b64（明文，仅公钥）"
  [IO.File]::WriteAllText($PubOut, $PubB64 + "`n", (New-Object Text.UTF8Encoding($false)))

  Write-Host "✅ 完成:"
  Write-Host "   secrets-vault\ota-keys.enc  （私钥 + 公钥 + update 主密钥，已加密）"
  Write-Host "   secrets-vault\ota-pub.b64   （公钥 base64，可分发）"
  Write-Host "   提醒: update_master_b64 需人工注入官网 api/update/keys.php 的 master 字段"
}
finally {
  if (Test-Path $TmpDir) { Remove-Item -Recurse -Force $TmpDir -ErrorAction SilentlyContinue }
}
