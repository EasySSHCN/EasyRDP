# pool-fix-009.ps1 —— R-20261119-009 回写完成发布（修复代码已随 v0.4.1 实际带出）
$ErrorActionPreference = 'Stop'
$root = 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP'
$poolPath = "$root\reqpool\requirements-pool.json"
$now = '2026-11-20T12:00:00+08:00'
$pool = Get-Content $poolPath -Raw -Encoding UTF8 | % { $_.TrimStart([char]0xFEFF) } | ConvertFrom-Json
foreach ($it in $pool.items) {
    if ($it.id -eq 'R-20261119-009') {
        $it.status = '完成发布'
        $it | Add-Member -NotePropertyName releaseVersion -NotePropertyValue '0.4.1' -Force
        $it.statusHistory = @($it.statusHistory) + @(@{
            status = '完成发布'; at = $now
            note = '随 v0.4.1 发布收口：修复代码（settings.js 横幅点击处理器先强制 check_update 再 download_and_install）在 v0.3.14 期已合入，随 v0.4.1 构建实际带出（545-558 行代码与注释线上包确认）'
        })
    }
}
[IO.File]::WriteAllText($poolPath, ($pool | ConvertTo-Json -Depth 100), (New-Object Text.UTF8Encoding $true))
Add-Content -Path "$root\reqpool\pool-add.log" -Value "- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') v0.4.1 补充回写：R-20261119-009 完成发布（修复随 v0.4.1 带出）" -Encoding UTF8
$chk = Get-Content $poolPath -Raw -Encoding UTF8 | % { $_.TrimStart([char]0xFEFF) } | ConvertFrom-Json
$rest = @($chk.items | Where-Object { $_.status -in '记录','待开发','开发中' })
"回写完成: 非终态 $($rest.Count) 条($((($rest | ForEach-Object id) -join ',')))"
