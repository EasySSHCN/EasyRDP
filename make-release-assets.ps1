# Build v0.1.0 release assets (ASCII-only)
# Outputs into EasyRDP\easyrdp-client\dist\ :
#   EasyRDP-0.1.0-win32-x64-portable.zip   (portable exe + FreeRDP license)
#   ..\..\EasyRDP-src-0.1.0.tar.gz         (full source, no build dirs)
#   dist\SHA256SUMS.txt
$ErrorActionPreference = 'Stop'
$WS   = 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP'
$DIST = "$WS\easyrdp-client\dist"
$Exe  = "$DIST\EasyRDP-0.1.0-portable.exe"

if (-not (Test-Path $Exe)) { throw "missing $Exe" }

# 1) portable zip
$Stage = "$DIST\EasyRDP-0.1.0-win32-x64-portable"
if (Test-Path $Stage) { Remove-Item $Stage -Recurse -Force }
New-Item $Stage -ItemType Directory -Force | Out-Null
Copy-Item $Exe $Stage
Copy-Item "$DIST\LICENSE-FreeRDP.txt" $Stage -ErrorAction SilentlyContinue
$Zip = "$DIST\EasyRDP-0.1.0-win32-x64-portable.zip"
if (Test-Path $Zip) { Remove-Item $Zip -Force }
Compress-Archive -Path "$Stage\*" -DestinationPath $Zip
Remove-Item $Stage -Recurse -Force

# 2) source tarball (exclude build outputs)
$SrcTar = "$WS\EasyRDP-src-0.1.0.tar.gz"
if (Test-Path $SrcTar) { Remove-Item $SrcTar -Force }
tar -czf $SrcTar -C (Split-Path -Parent $WS) --exclude 'EasyRDP/easyrdp-client/target' --exclude 'EasyRDP/easyrdp-gateway/target' --exclude 'EasyRDP/easyrdp-client/crates/easyrdp-rdp/target' --exclude 'node_modules' --exclude 'frames-debug' EasyRDP

# 3) SHA256SUMS
$files = @($Exe, $Zip, "$WS\dist-gateway\EasyRDP-Gateway-0.1.0-win-x64.zip", $SrcTar)
$lines = foreach ($f in $files) {
    $h = (Get-FileHash $f -Algorithm SHA256).Hash.ToLower()
    "$h  $(Split-Path -Leaf $f)"
}
$lines | Set-Content -Encoding ASCII "$DIST\SHA256SUMS.txt"
Get-Content "$DIST\SHA256SUMS.txt"
Get-ChildItem $Exe, $Zip, $SrcTar | Select Name, Length
