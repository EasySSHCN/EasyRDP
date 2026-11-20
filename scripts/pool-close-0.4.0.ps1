$ErrorActionPreference = 'Stop'
$poolPath = 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP\reqpool\requirements-pool.json'
$bytes = [IO.File]::ReadAllBytes($poolPath)
$hasBom = $bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF
$text = [Text.Encoding]::UTF8.GetString($bytes)
if ($hasBom) { $text = $text.Substring(1) }
$pool = $text | ConvertFrom-Json
$now = '2026-11-19T14:50:00+08:00'
foreach ($id in @('R-20261119-002','R-20261119-003','R-20261119-004','R-20261119-005')) {
    $item = $pool.items | Where-Object { $_.id -eq $id }
    if (-not $item) { throw "missing $id" }
    $item.status = '完成发布'
    $item | Add-Member -NotePropertyName releaseVersion -NotePropertyValue '0.4.0' -Force
    $hist = [System.Collections.Generic.List[object]]@($item.statusHistory)
    $hist.Add([pscustomobject]@{ at = $now; status = '完成发布'; note = 'v0.4.0 里程碑发布（GitHub/Gitee Release + 官网 8b 校验全过 + OTA latest.json 已上线）' })
    $item.statusHistory = $hist
}
$pool.meta.milestoneCount = @($pool.items).Count
if ($pool.meta.milestoneCount -ne 61) { Write-Warning "milestoneCount=$($pool.meta.milestoneCount)（预期 61，请核对）" }
$enc = New-Object System.Text.UTF8Encoding($hasBom)
[IO.File]::WriteAllText($poolPath, ($pool | ConvertTo-Json -Depth 10), $enc)
Write-Output "POOL_OK milestoneCount=$($pool.meta.milestoneCount)"
