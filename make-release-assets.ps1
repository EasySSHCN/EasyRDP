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
$Built = "$WS\easyrdp-client\target\release\easyrdp-app.exe"
if (-not (Test-Path $Built)) { throw "missing $Built (run package-release.ps1 first)" }
$Exe = "$DIST\EasyRDP-$V-portable.exe"
Copy-Item $Built $Exe -Force

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

# 3) source tarball (exclude build outputs; MUST exclude secrets-vault/signing -
#    permanent rule: all published source packages MUST be sanitized)
$SrcTar = "$WS\EasyRDP-src-$V.tar.gz"
if (Test-Path $SrcTar) { Remove-Item $SrcTar -Force }
tar -czf $SrcTar -C (Split-Path -Parent $WS) --exclude 'EasyRDP/easyrdp-client/target' --exclude 'EasyRDP/easyrdp-gateway/target' --exclude 'EasyRDP/easyrdp-client/crates/easyrdp-rdp/target' --exclude 'node_modules' --exclude 'frames-debug' --exclude 'EasyRDP/secrets-vault' --exclude 'EasyRDP/signing' --exclude 'EasyRDP/reqpool' EasyRDP
# sanitization gate: fail if any secret material made it into the tarball
$leak = tar -tzf $SrcTar | Select-String -Pattern 'secrets-vault|signing|\.enc$|/key$|id_ed25519|reqpool' | Select-Object -First 1
if ($leak) { throw "SANITIZATION GATE FAILED: source tarball contains $($leak.Line)" }

# 4) SHA256SUMS
$files = @($Exe, $Zip, $GwZip, $SrcTar)
$lines = foreach ($f in $files) {
    $h = (Get-FileHash $f -Algorithm SHA256).Hash.ToLower()
    "$h  $(Split-Path -Leaf $f)"
}
$lines | Set-Content -Encoding ASCII "$DIST\SHA256SUMS.txt"
Get-Content "$DIST\SHA256SUMS.txt"
Get-ChildItem $Exe, $Zip, $GwZip, $SrcTar | Select Name, Length
