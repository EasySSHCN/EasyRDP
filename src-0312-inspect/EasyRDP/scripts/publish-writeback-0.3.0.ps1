# publish-writeback-0.3.0.ps1 —— v0.3.0 发布完成后池状态回写（发布会话专用）
param([string]$Version = '0.3.0')
$ErrorActionPreference = 'Stop'
$root = 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP'
$poolPath = "$root\reqpool\requirements-pool.json"
$pool = Get-Content $poolPath -Raw -Encoding UTF8 | ConvertFrom-Json
$now = '2026-11-17T07:20:00+08:00'
$ids = @('R-20261116-023','R-20261116-024','R-20261116-025','R-20261116-026','R-20261117-001','R-20261117-002')
$notes = @{
    'R-20261116-023' = '随 v0.3.0 发布：GHOSTFIX4/5 方块残影根治（增量管线默认停用+呈现层帧世代计数），用户真机确认解决'
    'R-20261116-024' = '随 v0.3.0 发布：同上，拖动残影随增量管线停用消除，用户真机确认解决'
    'R-20261116-025' = '随 v0.3.0 发布：同上，按钮悬停残影消除，用户真机确认解决'
    'R-20261116-026' = '随 v0.3.0 发布：同上，光标拖尾残影消除，用户真机确认解决'
    'R-20261117-001' = '随 v0.3.0 发布：SAS 槽改组合键下拉菜单（内置 8 项含发送 Win 键）+ 设置中心「组合键」全局自定义分组'
    'R-20261117-002' = '随 v0.3.0 发布：连接表单底部改连接状态友好提示区（连接中/已连接/失败/断开附原因）'
}
$pool = Get-Content $poolPath -Raw -Encoding UTF8 | ConvertFrom-Json
foreach ($it in $pool.items) {
    if ($it.id -in $ids) {
        $it.status = '完成发布'
        $it | Add-Member -NotePropertyName releaseVersion -NotePropertyValue $Version -Force
        $it.statusHistory = @($it.statusHistory) + @(@{
            status = '完成发布'; at = $now; note = $notes[$it.id]
        })
    }
}
Copy-Item $poolPath "$poolPath.bak-030" -Force
[IO.File]::WriteAllText($poolPath, ($pool | ConvertTo-Json -Depth 100), (New-Object Text.UTF8Encoding $true))
Add-Content -Path "$root\reqpool\pool-add.log" -Value "- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') v0.3.0 发布回写：6 条完成发布（023-026 残影根治 + 001/002 新功能）" -Encoding UTF8
$chk = Get-Content $poolPath -Raw -Encoding UTF8 | ConvertFrom-Json
$done = @($chk.items | Where-Object { $_.releaseVersion -eq $Version })
"回写完成: v$Version 完成发布 $($done.Count) 条; 剩余记录/待开发 $((@($chk.items | Where-Object { $_.status -in '记录','待开发' })).Count) 条; 池总数 $($chk.items.Count)"
