﻿# ============================================================================
# ota-package.ps1 —— 生成 OTA 更新清单签名 + 加密升级包（需求 R-20261116-005）
# ----------------------------------------------------------------------------
# 用法（发布会话第 9/9 步由 release.sh 自动调用；也可手动）：
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\ota-package.ps1 -Version 0.1.6
#
# 前置：secrets-vault\ota-keys.enc + key 已由 ota-keygen.ps1 生成（本脚本不生成密钥）。
# 输入：dist\EasyRDP-<ver>-portable.exe（release.sh 第 3 步已校验存在）
# 产物：
#   easyrdp-site\api\update\latest.json
#       字段（共 5 个）：latestVersion / file / notes / sha256 / sig
#       sig = Ed25519 私钥对「清单规范 JSON 串」的签名（base64）。
#       规范 JSON 串 = {"latestVersion":"..","file":"..","notes":"..","sha256":".."}
#       （固定四字段、固定顺序、紧凑无空格、UTF-8，notes 按 JSON 转义；
#         客户端验签时按同一规则重建该串，详见 docs\方案-在线更新机制.md）
#   easyrdp-site\downloads\EasyRDP-<ver>-portable.exe.enc
#       AES-256-CTR 加密。文件密钥（32 字节）= HKDF-SHA256(ikm=update_master,
#         salt=0^32, info="update-file:"+文件名+明文sha256, L=32)
#       CTR 初始向量（16 字节）= HKDF-SHA256(同 ikm/salt,
#         info="update-file-iv:"+文件名+明文sha256, L=16)
#       完整性不靠加密层 HMAC，而是客户端解密后与 manifest.sha256 比对（详见方案文档）。
#
# 实现说明：
#   - Ed25519 签名 PS5.1/.NET 不支持 → 通过 openssl pkeyutl -sign -rawin 实现
#     （原生 PATH openssl，缺则回退 WSL bash openssl，与 ota-keygen.ps1 同一套封装）。
#   - AES-256-CTR：PS5.1 无原生 CTR（.NET SystemCore 只有 CBC）→ 统一走
#     bash+openssl enc -aes-256-ctr -K <hex32> -iv <hex16>（-K 需 hex 原始密钥）。
#   - HKDF-SHA256 在脚本内用 .NET HMACSHA256 实现（PS5.1 兼容）。
# ============================================================================

param(
  [Parameter(Mandatory = $true)][string]$Version
)

$ErrorActionPreference = 'Stop'

$Root   = Split-Path -Parent $PSScriptRoot
$Vault  = Join-Path $Root 'secrets-vault'
$EncIn  = Join-Path $Vault 'ota-keys.enc'
$KeyIn  = Join-Path $Vault 'key'
$Exe    = Join-Path $Root "dist\EasyRDP-$Version-portable.exe"
$Readme = Join-Path $Root 'README.md'
$SiteUpdateDir = Join-Path $Root 'easyrdp-site\api\update'
$SiteDlDir     = Join-Path $Root 'easyrdp-site\downloads'
$LatestJson    = Join-Path $SiteUpdateDir 'latest.json'
$OutEnc        = Join-Path $SiteDlDir "EasyRDP-$Version-portable.exe.enc"
$FileName      = "EasyRDP-$Version-portable.exe"

if (-not (Test-Path $EncIn)) { throw "缺少 secrets-vault\ota-keys.enc（先运行 ota-keygen.ps1）" }
if (-not (Test-Path $KeyIn)) { throw "缺少 secrets-vault\key" }
if (-not (Test-Path $Exe))   { throw "缺少待加密产物: $Exe" }

# ---------- openssl 封装（同 ota-keygen.ps1） ----------
$script:OsslMode = $null
if (Get-Command openssl.exe -ErrorAction SilentlyContinue) {
  $script:OsslMode = 'native'
} elseif (Get-Command wsl.exe -ErrorAction SilentlyContinue) {
  $script:OsslMode = 'wsl'
} else {
  throw '未找到 openssl（PATH）也无法回退 WSL——请安装其中之一后重试'
}
function ConvertTo-OsslPath([string]$Path) {
  if ($script:OsslMode -eq 'native') { return $Path }
  $p = (wsl.exe -- wslpath -a $Path 2>$null | Select-Object -First 1)
  if (-not $p) { throw "wslpath 转换失败: $Path" }
  return $p.Trim()
}
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

$TmpDir = Join-Path $env:TEMP ("ota-package-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $TmpDir | Out-Null

function Remove-TmpDir { if ($TmpDir -and (Test-Path $TmpDir)) { Remove-Item -Recurse -Force $TmpDir -ErrorAction SilentlyContinue } }

# ---------- 1. 解密密钥库 ----------
try {
  Write-Host "==> 读取 secrets-vault\ota-keys.enc（openssl aes-256-cbc -pbkdf2 -d）"
  $Plain = Join-Path $TmpDir 'ota-keys.plain'
  Invoke-Ossl ("enc -aes-256-cbc -d -pbkdf2 -in `"" + (ConvertTo-OsslPath $EncIn) + "`" -out `"" + (ConvertTo-OsslPath $Plain) + "`" -pass file:`"" + (ConvertTo-OsslPath $KeyIn) + "`"") | Out-Null
  $keys = @{}
  foreach ($line in [IO.File]::ReadAllLines($Plain)) {
    $idx = $line.IndexOf(': ')
    if ($idx -gt 0) { $keys[$line.Substring(0, $idx)] = $line.Substring($idx + 2).Trim() }
  }
  foreach ($k in @('ota_ed25519_priv_b64', 'ota_ed25519_pub_b64', 'update_master_b64')) {
    if (-not $keys[$k]) { throw "ota-keys.enc 内容缺失字段: $k" }
  }

  # ---------- 2. 明文 SHA256 + notes ----------
  Write-Host "==> 计算 portable.exe SHA256"
  $Sha256 = (Get-FileHash -Algorithm SHA256 -Path $Exe).Hash.ToLower()
  $notes = ''
  if (Test-Path $Readme) {
    # 与 release.sh 第 5 步同源：README「### v<ver>」小节正文（去掉内层 ### 小节）
    $inSec = $false; $buf = @()
    foreach ($line in [IO.File]::ReadAllLines($Readme)) {
      if ($line -like "### v$Version*") { $inSec = $true; continue }
      if ($inSec) {
        if ($line -like '### *') { break }
        $buf += $line
      }
    }
    $notes = (($buf -join "`n").Trim())
  }
  if (-not $notes) { $notes = "EasyRDP $Version" }

  # ---------- 3. 清单规范 JSON 串 + Ed25519 签名（openssl pkeyutl -rawin） ----------
  Write-Host "==> 生成清单 + Ed25519 签名"
  function ConvertTo-JsonStr([string]$s) {
    $r = $s -replace '\\', '\\\\' -replace '"', '\"' -replace "`r", '\r' -replace "`n", '\n' -replace "`t", '\t'
    # 控制字符兜底转义 \u00XX
    $r = [regex]::Replace($r, '[\x00-\x1f]', { param($m) '\u{0:x4}' -f [int][char]$m.Value[0] })
    return $r
  }
  $canonical = '{"latestVersion":"' + (ConvertTo-JsonStr $Version) + '","file":"' + (ConvertTo-JsonStr $FileName) + '","notes":"' + (ConvertTo-JsonStr $notes) + '","sha256":"' + $Sha256 + '"}'
  $ManFile = Join-Path $TmpDir 'manifest.canonical'
  [IO.File]::WriteAllText($ManFile, $canonical, (New-Object Text.UTF8Encoding($false)))

  # 原始 32 字节私钥 → PKCS8 PEM（固定前缀 302e020100300506032b657004220420 + 种子）
  $privRaw = [Convert]::FromBase64String($keys['ota_ed25519_priv_b64'])
  if ($privRaw.Length -ne 32) { throw 'Ed25519 私钥 base64 不是 32 字节' }
  $der = New-Object byte[] 48
  [byte[]]$prefix = 0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20
  [Array]::Copy($prefix, $der, 16)
  [Array]::Copy($privRaw, 0, $der, 16, 32)
  $b64 = [Convert]::ToBase64String($der)
  $pemLines = @('-----BEGIN PRIVATE KEY-----')
  for ($i = 0; $i -lt $b64.Length; $i += 64) { $pemLines += $b64.Substring($i, [Math]::Min(64, $b64.Length - $i)) }
  $pemLines += '-----END PRIVATE KEY-----'
  $PrivPem = Join-Path $TmpDir 'ota-priv.pem'
  [IO.File]::WriteAllLines($PrivPem, $pemLines)

  $SigFile = Join-Path $TmpDir 'manifest.sig'
  Invoke-Ossl ("pkeyutl -sign -rawin -inkey `"" + (ConvertTo-OsslPath $PrivPem) + "`" -in `"" + (ConvertTo-OsslPath $ManFile) + "`" -out `"" + (ConvertTo-OsslPath $SigFile) + "`"") | Out-Null
  $Sig = [Convert]::ToBase64String([IO.File]::ReadAllBytes($SigFile))

  # ---------- 4. latest.json（5 字段；UTF-8 无 BOM，避免 PHP json_decode/客户端解析失败） ----------
  # 组装：规范串已含前 4 字段（latestVersion/file/notes/sha256），收尾补 sig
  $json = $canonical.Substring(0, $canonical.Length - 1) + ',"sig":"' + $Sig + '"}'
  New-Item -ItemType Directory -Force -Path $SiteUpdateDir | Out-Null
  [IO.File]::WriteAllText($LatestJson, $json + "`n", (New-Object Text.UTF8Encoding($false)))
  Write-Host "  ✓ easyrdp-site\api\update\latest.json（latestVersion=$Version, sha256=$($Sha256.Substring(0,12))…）"

  # ---------- 5. HKDF-SHA256 派生文件密钥与 CTR IV ----------
  Write-Host "==> HKDF-SHA256 派生文件密钥/CTR IV"
  function New-HmacBytes([byte[]]$Key, [byte[]]$Data) {
    $h = New-Object System.Security.Cryptography.HMACSHA256 -ArgumentList (, $Key)
    try { return $h.ComputeHash($Data) } finally { $h.Dispose() }
  }
  function Get-HkdfSha256([byte[]]$Ikm, [byte[]]$Info, [int]$Length) {
    $salt = New-Object byte[] 32          # RFC5869：缺省 salt = HashLen 个 0
    $prk = New-HmacBytes $salt $Ikm
    $okm = New-Object byte[] $Length
    $t = New-Object byte[] 0
    $pos = 0; $counter = 1
    while ($pos -lt $Length) {
      $input = New-Object byte[] ($t.Length + $Info.Length + 1)
      [Array]::Copy($t, $input, $t.Length)
      [Array]::Copy($Info, 0, $input, $t.Length, $Info.Length)
      $input[$input.Length - 1] = [byte]$counter
      $t = New-HmacBytes $prk $input
      $n = [Math]::Min(32, $Length - $pos)
      [Array]::Copy($t, 0, $okm, $pos, $n)
      $pos += $n; $counter++
    }
    return $okm
  }
  $master = [Convert]::FromBase64String($keys['update_master_b64'])
  if ($master.Length -ne 32) { throw 'update_master_b64 不是 32 字节' }
  $infoBytes  = [Text.Encoding]::UTF8.GetBytes("update-file:$FileName$Sha256")
  $infoBytesIv = [Text.Encoding]::UTF8.GetBytes("update-file-iv:$FileName$Sha256")
  $fileKey = Get-HkdfSha256 $master $infoBytes 32
  $ctrIv   = Get-HkdfSha256 $master $infoBytesIv 16
  $keyHex = ([BitConverter]::ToString($fileKey)) -replace '-', ''
  $ivHex  = ([BitConverter]::ToString($ctrIv)) -replace '-', ''

  # ---------- 6. AES-256-CTR 加密（bash+openssl enc -aes-256-ctr；PS5.1 无原生 CTR） ----------
  Write-Host "==> AES-256-CTR 加密升级包"
  New-Item -ItemType Directory -Force -Path $SiteDlDir | Out-Null
  # openssl enc 不接受 0777 权限的 -pass file → 这里用 -K/-iv hex 直传密钥（仅进程内可见），
  # 避免落盘明文密钥文件；大文件经文件参数流式处理。
  Invoke-Ossl ("enc -aes-256-ctr -K $keyHex -iv $ivHex -in `"" + (ConvertTo-OsslPath $Exe) + "`" -out `"" + (ConvertTo-OsslPath $OutEnc) + "`"") | Out-Null
  $encSize = (Get-Item $OutEnc).Length
  Write-Host "  ✓ easyrdp-site\downloads\$FileName.enc（$([Math]::Round($encSize / 1MB, 1)) MB，与明文等长 CTR 密文）"

  Write-Host "✅ OTA 加密升级包生成完成: v$Version"
}
finally {
  Remove-TmpDir
}
