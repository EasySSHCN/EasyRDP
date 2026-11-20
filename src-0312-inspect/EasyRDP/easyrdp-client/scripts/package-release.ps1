# EasyRDP Windows release packaging script
#
# Steps:
#   1. Set FreeRDP build env (PKG_CONFIG_PATH / pkgconf)
#   2. cargo build --release -p easyrdp-app --features custom-protocol,native
#   3. Stage dist\EasyRDP-win64\: exe + FreeRDP license file
#   4. Compress to dist\EasyRDP-win64.zip
#
# FreeRDP 2.11.2 is statically linked (vcpkg x64-windows-static-md): the zip
# carries ONLY the exe (no freerdp2/freerdp-client2/winpr2/OpenSSL DLLs).
# NOTE: OpenSSL runtime DLLs are still NOT needed - the static libcrypto/libssl
# are linked into the exe; OpenSSL remains dynamically linked only against the
# CRT (vcruntime/ucrtbase, present on every Windows 10+).
#
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package-release.ps1
# NOTE: keep this file ASCII-only (Windows PowerShell 5.1 misreads UTF-8
# without BOM and breaks CJK strings).

param(
    [string]$VcpkgRoot  = 'B:\tools\EasyRDP\vcpkg',
    [string]$PkgconfRoot = 'B:\tools\EasyRDP\pkgconf-msi\PFiles64\pkgconf-3.0.10',
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot   # repo root (easyrdp-client)
Set-Location $Root

# static FreeRDP libs must exist (freerdp DLLs are no longer shipped or needed)
$VcpkgLib = Join-Path $VcpkgRoot 'installed\x64-windows-static-md\lib'
foreach ($lib in 'freerdp-client2.lib','freerdp2.lib','winpr2.lib','libcrypto.lib','libssl.lib') {
    if (-not (Test-Path (Join-Path $VcpkgLib $lib))) {
        throw "vcpkg static FreeRDP lib not found: $VcpkgLib\$lib (see docs\WINDOWS-FREERDP-SETUP.md)"
    }
}

if (-not $SkipBuild) {
    # session-level build env (docs/WINDOWS-FREERDP-SETUP.md; static-md triplet)
    $env:PKG_CONFIG_PATH = Join-Path $VcpkgRoot 'installed\x64-windows-static-md\lib\pkgconfig'
    $env:PATH = "$PkgconfRoot;$env:PATH"
    cargo build --release -p easyrdp-app --features custom-protocol,native
    if ($LASTEXITCODE -ne 0) { throw "release build failed (exit $LASTEXITCODE)" }
}

# -- stage release directory --
$Exe = Join-Path $Root 'target\release\easyrdp-app.exe'
if (-not (Test-Path $Exe)) { throw "missing $Exe (build first)" }

# FreeRDP 2.11.2 license (Apache-2.0): carried by the vcpkg buildtrees source tree.
$FreerdpLicense = Get-ChildItem (Join-Path $VcpkgRoot 'buildtrees\freerdp\src') -Recurse -Filter 'LICENSE' -ErrorAction SilentlyContinue |
    Select-Object -First 1 -ExpandProperty FullName

$Stage = Join-Path $Root 'dist\EasyRDP-win64'
if (Test-Path $Stage) { Remove-Item $Stage -Recurse -Force }
New-Item $Stage -ItemType Directory -Force | Out-Null

# statically linked build: exe only (FreeRDP/WinPR/OpenSSL all linked in)
Copy-Item $Exe $Stage
if ($FreerdpLicense) {
    Copy-Item $FreerdpLicense (Join-Path $Stage 'LICENSE-FreeRDP.txt')
} else {
    Write-Warning 'FreeRDP LICENSE not found in buildtrees source tree; release zip lacks license file'
}
# OpenSSL 3.x is Apache-2.0 as well; note the source if vcpkg ships no license file.
Write-Output "Stage directory: $Stage"
Get-ChildItem $Stage | Format-Table Name, Length -AutoSize

# -- compress --
$Zip = Join-Path $Root 'dist\EasyRDP-win64.zip'
if (Test-Path $Zip) { Remove-Item $Zip -Force }
Compress-Archive -Path (Join-Path $Stage '*') -DestinationPath $Zip
Write-Output "Release zip: $Zip"
