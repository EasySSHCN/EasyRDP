# Robust zip builder: per-file skip on error, shared-read open (works for running DLLs)
param(
  [Parameter(Mandatory=$true)][string]$SourceDir,
  [Parameter(Mandatory=$true)][string]$DestZip
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
if (Test-Path $DestZip) { Remove-Item $DestZip -Force }
$fs = [System.IO.File]::Open($DestZip, [System.IO.FileMode]::CreateNew)
$zip = New-Object System.IO.Compression.ZipArchive($fs, [System.IO.Compression.ZipArchiveMode]::Create)
$level = [System.IO.Compression.CompressionLevel]::Fastest
$ok = 0; $skip = New-Object System.Collections.Generic.List[string]
try {
  $srcFull = (Get-Item $SourceDir).FullName.TrimEnd('\') + '\'
  $files = Get-ChildItem $SourceDir -Recurse -File -Force
  $i = 0
  foreach ($f in $files) {
    $i++
    if ($i % 20000 -eq 0) { Write-Host ("  ... {0} files" -f $i) }
    $rel = $f.FullName.Substring($srcFull.Length) -replace '\\','/'
    if ($f.LastWriteTime -lt [datetime]'1980-01-02') { $f.LastWriteTime = [datetime]'2026-01-01' }
    try {
      $entry = $zip.CreateEntry($rel, $level)
      $es = $entry.Open()
      $in = [System.IO.File]::Open($f.FullName, [System.IO.FileMode]::Open,
              [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete)
      try { $in.CopyTo($es) } finally { $in.Dispose(); $es.Dispose() }
      $ok++
    } catch {
      $skip.Add("$rel :: $($_.Exception.Message)")
    }
  }
} finally {
  $zip.Dispose(); $fs.Dispose()
}
Write-Host ("OK {0} files, SKIPPED {1}" -f $ok, $skip.Count)
$skip | ForEach-Object { Write-Host "  SKIP: $_" }
$z = Get-Item $DestZip
Write-Host ("ZIP: {0}  ({1:N0} MB)" -f $DestZip, ($z.Length/1MB))
