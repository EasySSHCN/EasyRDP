# publish-writeback-0.3.2.ps1 —— v0.3.2 发布完成后池状态回写（发布会话专用）
$ErrorActionPreference = 'Stop'
$root = 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP'
$poolPath = "$root\reqpool\requirements-pool.json"
$now = '2026-11-17T18:10:00+08:00'
# 004-007 追溯随 v0.3.1（代码与 README v0.3.1 小节已含，仅补池状态）；
# 003/008/010 随 v0.3.2；009 残影反馈用户确认此前已解决 → 已关闭不发布
$map = @(
    @{ ids = @('R-20261117-004','R-20261117-005','R-20261117-006','R-20261117-007'); ver = '0.3.1' },
    @{ ids = @('R-20261117-003','R-20261117-008','R-20261117-010'); ver = '0.3.2' }
)
$notes = @{
    'R-20261117-003' = '随 v0.3.2 发布：取消按钮右侧 #msg 提示，flash 全部整合到底部 conn-hint（防覆盖 token）'
    'R-20261117-004' = '随 v0.3.1 发布：组合键持键 80ms 发送（send_vk_combo），追溯回写'
    'R-20261117-005' = '随 v0.3.1 发布：NumpadDecimal NumLock ON 发 0x34 句点码位，追溯回写'
    'R-20261117-006' = '随 v0.3.1 发布：SAS 槽文案改「发送」，追溯回写'
    'R-20261117-007' = '随 v0.3.1 发布：关闭按钮字形改字母 X，追溯回写'
    'R-20261117-008' = '随 v0.3.2 发布：真机复现确认 v0.3.1 仍失败，定位根因=客户端规范串重建被 serde_json Map 字母序打乱字段顺序（file 在 latestVersion 前），改为约定顺序手工拼接；Rust 语义端到端探针现役清单验签 VALID + 字段顺序回归测试'
    'R-20261117-010' = '随 v0.3.2 关闭：诉求1（Win 键无效）与 R-20261117-004 同根因已随 v0.3.1 修复；诉求2（连接后 UI 不隐藏+多窗口）已由 R-20261116-007 多会话并行实现'
    'R-20261117-009' = '已关闭（用户 2026-11-17 指示）：残影问题此前已解决（R-20261116-023~026 随 v0.3.0 根治），本条不再继续'
}
$pool = Get-Content $poolPath -Raw -Encoding UTF8 | % { $_.TrimStart([char]0xFEFF) } | ConvertFrom-Json
foreach ($g in $map) {
    foreach ($it in $pool.items) {
        if ($it.id -in $g.ids) {
            $it.status = '完成发布'
            $it | Add-Member -NotePropertyName releaseVersion -NotePropertyValue $g.ver -Force
            $it.statusHistory = @($it.statusHistory) + @(@{
                status = '完成发布'; at = $now; note = $notes[$it.id]
            })
        }
    }
}
foreach ($it in $pool.items) {
    if ($it.id -eq 'R-20261117-009') {
        $it.status = '已关闭'
        $it.statusHistory = @($it.statusHistory) + @(@{
            status = '已关闭'; at = $now; note = $notes[$it.id]
        })
    }
}
# 里程碑基线：发布完成时 milestoneCount = 当时池条目总数（MILE-20261116 规则）
$total = $pool.items.Count
$pool.meta | Add-Member -NotePropertyName milestoneCount -NotePropertyValue $total -Force
Copy-Item $poolPath "$poolPath.bak-032" -Force
[IO.File]::WriteAllText($poolPath, ($pool | ConvertTo-Json -Depth 100), (New-Object Text.UTF8Encoding $true))
Add-Content -Path "$root\reqpool\pool-add.log" -Value "- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') v0.3.2 发布回写：003/008/010 完成发布 v0.3.2，004-007 追溯 v0.3.1，009 已关闭（残影此前已解决）；milestoneCount=$total" -Encoding UTF8
$chk = Get-Content $poolPath -Raw -Encoding UTF8 | % { $_.TrimStart([char]0xFEFF) } | ConvertFrom-Json
$done = @($chk.items | Where-Object { $_.status -eq '完成发布' })
$rest = @($chk.items | Where-Object { $_.status -in '记录','待开发','开发中' })
"回写完成: 完成发布 $($done.Count) 条; 剩余 $($rest.Count) 条($((($rest | ForEach-Object id) -join ','))); 池总数 $($chk.items.Count); milestoneCount=$($chk.meta.milestoneCount)"
