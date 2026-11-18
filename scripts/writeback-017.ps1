# writeback-017.ps1 —— R-20261116-017 状态回写（修复已随 v0.2.1 发布，池状态未同步的历史脏数据）
$ErrorActionPreference = 'Stop'
$root = 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP'
$poolPath = "$root\reqpool\requirements-pool.json"
$now = '2026-11-18T06:30:00+08:00'
$pool = Get-Content $poolPath -Raw -Encoding UTF8 | % { $_.TrimStart([char]0xFEFF) } | ConvertFrom-Json
foreach ($it in $pool.items) {
    if ($it.id -eq 'R-20261116-017') {
        $it.status = '完成发布'
        $it | Add-Member -NotePropertyName releaseVersion -NotePropertyValue '0.2.1' -Force
        $it.statusHistory = @($it.statusHistory) + @(@{
            status = '完成发布'; at = $now
            note = '状态回写（脏数据清理）：其修复（边缘 resize 热区+背板缩放/Delta-全帧时序反转残影根治）已随 v0.2.1 发布并有真机确认，docs 需求变更记录池 F-R1-17 早已记录，仅池状态漏回写'
        })
    }
}
[IO.File]::WriteAllText($poolPath, ($pool | ConvertTo-Json -Depth 100), (New-Object Text.UTF8Encoding $true))
Add-Content -Path "$root\reqpool\pool-add.log" -Value "- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') R-20261116-017 状态回写完成发布 v0.2.1（脏数据清理）" -Encoding UTF8
$chk = Get-Content $poolPath -Raw -Encoding UTF8 | % { $_.TrimStart([char]0xFEFF) } | ConvertFrom-Json
$rest = @($chk.items | Where-Object { $_.status -notin '完成发布','已关闭','暂缓' })
"回写完成; 非终态 $($rest.Count) 条: $((($rest | ForEach-Object id) -join ', '))"
