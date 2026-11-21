# publish-writeback-0.3.3.ps1 —— v0.3.3 发布完成后池状态回写（发布会话专用）
$ErrorActionPreference = 'Stop'
$root = 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP'
$poolPath = "$root\reqpool\requirements-pool.json"
$now = '2026-11-17T22:05:00+08:00'
$pool = Get-Content $poolPath -Raw -Encoding UTF8 | % { $_.TrimStart([char]0xFEFF) } | ConvertFrom-Json
foreach ($it in $pool.items) {
    if ($it.id -eq 'R-20261117-011') {
        $it.status = '完成发布'
        $it | Add-Member -NotePropertyName releaseVersion -NotePropertyValue '0.3.3' -Force
        $it.statusHistory = @($it.statusHistory) + @(@{
            status = '完成发布'; at = $now
            note = '随 v0.3.3 发布：文本剪贴板协议顺序修正（Monitor Ready 后公告）+ 未就绪检测提示 + 文件剪贴板双向传输（CLIPRDR FileContents，FileGroupDescriptorW/FileContents 命名格式，分块 1MiB，落地下载\EasyRDP-Clipboard，签名去重防回环，≤128 文件/≤2GiB）。用户真机确认：文本双向、文件双向均正常'
        })
    }
}
# 里程碑基线：发布完成时 milestoneCount = 当时池条目总数（MILE-20261116 规则）
$total = $pool.items.Count
$pool.meta | Add-Member -NotePropertyName milestoneCount -NotePropertyValue $total -Force
Copy-Item $poolPath "$poolPath.bak-033" -Force
[IO.File]::WriteAllText($poolPath, ($pool | ConvertTo-Json -Depth 100), (New-Object Text.UTF8Encoding $true))
Add-Content -Path "$root\reqpool\pool-add.log" -Value "- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') v0.3.3 发布回写：R-20261117-011 完成发布（R-012 组合键按键捕获保持待开发）；milestoneCount=$total" -Encoding UTF8
$chk = Get-Content $poolPath -Raw -Encoding UTF8 | % { $_.TrimStart([char]0xFEFF) } | ConvertFrom-Json
$rest = @($chk.items | Where-Object { $_.status -in '记录','待开发','开发中' })
"回写完成: 池总数 $($chk.items.Count); 非终态 $($rest.Count) 条($((($rest | ForEach-Object id) -join ','))); milestoneCount=$($chk.meta.milestoneCount)"
