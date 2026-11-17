# EasyRDP disaster-recovery package builder (2026-11-15 fix50)
# ASCII-only for Windows PowerShell 5.1 compatibility.
# Contents: all sources + vcpkg FreeRDP components (installed static libs/headers,
#           freerdp 2.11.2 source tree needed by build.rs) + pkgconf + overlay-ports
#           + freerdp2-sys crate cache + release artifacts + restore doc.
param([string]$Dest = 'B:\EasyRDP-DR-20261115')
$ErrorActionPreference = 'Stop'
$WS   = 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP'
$TOOL = 'B:\tools\EasyRDP'

if (Test-Path $Dest) { throw "destination exists: $Dest" }
New-Item $Dest -ItemType Directory -Force | Out-Null

function Copy-Tree([string]$src, [string]$dst, [string[]]$xd) {
    $a = @($src, $dst, '/E', '/NFL', '/NDL', '/NJH', '/NP', '/R:1', '/W:1')
    foreach ($d in $xd) { $a += @('/XD', $d) }
    robocopy @a | Out-Null
    if ($LASTEXITCODE -ge 8) { throw "robocopy failed ($LASTEXITCODE): $src -> $dst" }
}

# 1) project sources (exclude build dirs; keep dist artifacts)
Copy-Tree $WS "$Dest\src\EasyRDP" @('target', 'node_modules')

# 2) vcpkg components
New-Item "$Dest\tools\EasyRDP\vcpkg" -ItemType Directory -Force | Out-Null
Copy-Tree "$TOOL\vcpkg\installed"              "$Dest\tools\EasyRDP\vcpkg\installed" @()
Copy-Tree "$TOOL\vcpkg\buildtrees\freerdp\src" "$Dest\tools\EasyRDP\vcpkg\buildtrees\freerdp\src" @()
Copy-Tree "$TOOL\vcpkg\ports"                  "$Dest\tools\EasyRDP\vcpkg\ports" @()
Copy-Tree "$TOOL\vcpkg\scripts"                "$Dest\tools\EasyRDP\vcpkg\scripts" @()
Copy-Item "$TOOL\vcpkg\vcpkg.exe"              "$Dest\tools\EasyRDP\vcpkg\vcpkg.exe"
Copy-Item "$TOOL\vcpkg\bootstrap-vcpkg.bat"    "$Dest\tools\EasyRDP\vcpkg\bootstrap-vcpkg.bat"
Copy-Item -Force "$TOOL\vcpkg\.vcpkg-root"     "$Dest\tools\EasyRDP\vcpkg\.vcpkg-root"
Copy-Item -Force "$TOOL\vcpkg\.gitignore"      "$Dest\tools\EasyRDP\vcpkg\.gitignore"

# 3) pkgconf / overlay-ports / crate cache / version pins
Copy-Tree "$TOOL\pkgconf-msi"        "$Dest\tools\EasyRDP\pkgconf-msi" @()
Copy-Tree "$TOOL\overlay-ports"      "$Dest\tools\EasyRDP\overlay-ports" @()
Copy-Tree "$TOOL\freerdp2-sys-0.2.0" "$Dest\tools\EasyRDP\freerdp2-sys-0.2.0" @()
foreach ($f in 'f2sys.crate', 'baseline.txt', 'freerdp-versions.json', 'pkgconf.msi') {
    if (Test-Path "$TOOL\$f") { Copy-Item "$TOOL\$f" "$Dest\tools\EasyRDP\$f" }
}

# 4) encrypted credentials vault (creds.enc + key; see secrets-vault\README.txt)
if (Test-Path "$WS\secrets-vault") {
    Copy-Tree "$WS\secrets-vault" "$Dest\src\EasyRDP\secrets-vault" @()
}

# 5) restore guide at package root (versioned in docs\RESTORE.md)
if (Test-Path "$WS\docs\RESTORE.md") {
    Copy-Item "$WS\docs\RESTORE.md" "$Dest\RESTORE.md" -Force
}

Write-Output "DR package assembled at $Dest"
$m = Get-ChildItem $Dest -Recurse -File | Measure-Object Length -Sum
Write-Output ("Total: {0:N0} MB, {1} files" -f ($m.Sum / 1MB), $m.Count)
