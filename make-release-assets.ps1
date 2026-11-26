# Build EasyRDP release assets (ASCII-only). Version-driven (v0.1.1+).
# Usage: powershell -ExecutionPolicy Bypass -File make-release-assets.ps1 [-Version 0.1.1]
#   Version defaults to easyrdp-client\src-tauri\tauri.conf.json "version".
# Outputs:
#   easyrdp-client\dist\EasyRDP-<V>-portable.exe
#   easyrdp-client\dist\EasyRDP-<V>-win32-x64-portable.zip
#   dist-gateway\EasyRDP-Gateway-<V>-win-x64.zip   (built if missing)
#   EasyRDP-src-<V>.tar.gz                          (sanitized, secrets gate)
#   easyrdp-client\dist\SHA256SUMS.txt
param(
    [string]$Version = '',
    [string]$VcpkgRoot = 'B:\tools\EasyRDP\vcpkg'
)
$ErrorActionPreference = 'Stop'
$WS   = 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP'
$DIST = "$WS\easyrdp-client\dist"
$CONF = "$WS\easyrdp-client\src-tauri\tauri.conf.json"
if (-not $Version) {
    $Version = ((Get-Content $CONF -Raw -Encoding UTF8 | ConvertFrom-Json).version)
    if (-not $Version) { throw 'cannot resolve version from tauri.conf.json; pass -Version' }
}
$V = $Version
Write-Output "Asset version: $V (from $CONF)"

# 0) portable exe: copy freshly built binary to versioned name
# HOTFIX-20261123 (OTA-LOOP): $Built 必须感知 CARGO_TARGET_DIR——CI/发布流程统一用
# C:\er-target 短路径（规避 MSVC D8050），旧实现硬编码 target\release 会拿到陈旧
# 旧版本二进制（实证：v0.5.1 资产内容是 0.5.0，用户 OTA 升级死循环）。并加
# FileVersion 硬校验：拷贝源的 PE 版本资源必须等于本次发布版本，否则终止。
$BuiltCandidates = @()
if ($env:CARGO_TARGET_DIR) { $BuiltCandidates += (Join-Path $env:CARGO_TARGET_DIR 'release\easyrdp-app.exe') }
$BuiltCandidates += 'C:\er-target\release\easyrdp-app.exe'
$BuiltCandidates += "$WS\easyrdp-client\target\release\easyrdp-app.exe"
$Built = $null
foreach ($c in $BuiltCandidates) {
    if ((Test-Path $c) -and ((Get-Item $c).LastWriteTime -gt (Get-Date).AddHours(-24))) { $Built = $c; break }
}
if (-not $Built) { foreach ($c in $BuiltCandidates) { if (Test-Path $c) { $Built = $c; break } } }
if (-not $Built) { throw "missing easyrdp-app.exe in any candidate target dir (run package-release.ps1 first)" }
Write-Output "Portable source: $Built (LastWrite=$((Get-Item $Built).LastWriteTime.ToString('s')))"
$fv = (Get-Item $Built).VersionInfo.FileVersion
if ($fv -ne $V) {
    throw ("VERSION MISMATCH GUARD: {0} FileVersion={1} != release version {2} — stale build, aborting (OTA-LOOP hotfix)" -f $Built, $fv, $V)
}
$Exe = "$DIST\EasyRDP-$V-portable.exe"
Copy-Item $Built $Exe -Force
$fv2 = (Get-Item $Exe).VersionInfo.FileVersion
if ($fv2 -ne $V) { throw "post-copy verify failed: $Exe FileVersion=$fv2 != $V" }

# 1) portable zip
$Stage = "$DIST\EasyRDP-$V-win32-x64-portable"
if (Test-Path $Stage) { Remove-Item $Stage -Recurse -Force }
New-Item $Stage -ItemType Directory -Force | Out-Null
Copy-Item $Exe $Stage
Copy-Item "$DIST\LICENSE-FreeRDP.txt" $Stage -ErrorAction SilentlyContinue
$Zip = "$DIST\EasyRDP-$V-win32-x64-portable.zip"
if (Test-Path $Zip) { Remove-Item $Zip -Force }
Compress-Archive -Path "$Stage\*" -DestinationPath $Zip
Remove-Item $Stage -Recurse -Force

# 2) gateway zip (build if missing)
$GwZip = "$WS\dist-gateway\EasyRDP-Gateway-$V-win-x64.zip"
if (-not (Test-Path $GwZip)) {
    Write-Output "Building gateway $V ..."
    Push-Location "$WS\easyrdp-gateway"
    cargo build --release
    if ($LASTEXITCODE -ne 0) { Pop-Location; throw 'gateway build failed' }
    Pop-Location
    $GwExe = "$WS\easyrdp-gateway\target\release\easyrdp-gateway.exe"
    if (-not (Test-Path $GwExe)) { throw "missing $GwExe" }
    $GwStage = "$WS\dist-gateway\EasyRDP-Gateway-$V-win-x64"
    if (Test-Path $GwStage) { Remove-Item $GwStage -Recurse -Force }
    New-Item $GwStage -ItemType Directory -Force | Out-Null
    Copy-Item $GwExe $GwStage
    Copy-Item "$WS\easyrdp-gateway\README.md" $GwStage -ErrorAction SilentlyContinue
    Copy-Item "$WS\easyrdp-gateway\config.example.toml" $GwStage -ErrorAction SilentlyContinue
    if (Test-Path $GwZip) { Remove-Item $GwZip -Force }
    Compress-Archive -Path "$GwStage\*" -DestinationPath $GwZip
    Remove-Item $GwStage -Recurse -Force
}

# 3) source tarball (written outside the tree to avoid self-inclusion; MUST exclude secrets)
#    TILEGHOST-20261116 fix: SrcTar was written inside $WS (tar self-included, 6.7GB).
#    Now written to the parent dir of EasyRDP.
$SrcTar = "$WS\..\EasyRDP-src-$V.tar.gz"
if (Test-Path $SrcTar) { Remove-Item $SrcTar -Force }
tar -czf $SrcTar -C (Split-Path -Parent $WS) --exclude 'EasyRDP/easyrdp-client/target' --exclude 'EasyRDP/easyrdp-gateway/target' --exclude 'EasyRDP/easyrdp-client/crates/easyrdp-rdp/target' --exclude 'EasyRDP/easyrdp-client/src-tauri/target' --exclude 'node_modules' --exclude 'frames-debug' --exclude 'EasyRDP/secrets-vault' --exclude 'EasyRDP/signing' --exclude 'EasyRDP/reqpool' --exclude 'EasyRDP/dist' --exclude 'EasyRDP/easyrdp-client/dist' --exclude 'EasyRDP/tools/ghost-e2e' --exclude 'EasyRDP/dist-gateway' --exclude 'EasyRDP/easyrdp-site/downloads' --exclude 'EasyRDP/easyrdp-site/id_ed25519*' --exclude 'EasyRDP/easyrdp-site/*.pem' --exclude 'EasyRDP/easyrdp-site/id_*' --exclude 'EasyRDP/EasyRDP-src-*.tar.gz' --exclude 'EasyRDP/EasyRDP-0.*-portable.zip' --exclude 'EasyRDP/EasyRDP-*portable' EasyRDP
# Sanitization gate: fail if secrets/signing/credentials appear in the tar
$leak = tar -tzf $SrcTar | Select-String -Pattern 'secrets-vault|signing|\.enc$|/key$|id_ed25519|reqpool' | Select-Object -First 1
if ($leak) { throw "SANITIZATION GATE FAILED: source tarball contains $($leak.Line)" }
# Move back to project root (path convention) and sanity-check size (<200MB)
$FinalTar = "$WS\EasyRDP-src-$V.tar.gz"
Move-Item $SrcTar $FinalTar -Force
$tarSize = (Get-Item $FinalTar).Length
if ($tarSize -gt 200MB) { throw "source tarball abnormally huge ($([math]::Round($tarSize/1MB))MB) - possible self-inclusion, abort" }
Write-Output "src tar: $([math]::Round($tarSize/1MB,1))MB"

# 4) SHA256SUMS
$files = @($Exe, $Zip, $GwZip, "$WS\EasyRDP-src-$V.tar.gz")
$lines = foreach ($f in $files) {
    $h = (Get-FileHash $f -Algorithm SHA256).Hash.ToLower()
    "$h  $(Split-Path -Leaf $f)"
}
$lines | Set-Content -Encoding ASCII "$DIST\SHA256SUMS.txt"
Get-Content "$DIST\SHA256SUMS.txt"
Get-ChildItem $Exe, $Zip, $GwZip, "$WS\EasyRDP-src-$V.tar.gz" | Select Name, Length
