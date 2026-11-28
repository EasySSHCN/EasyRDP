# publish-writeback.ps1 —— v0.1.1 发布完成后池状态回写（发布会话专用）
param([string]$Version = '0.1.1')
$ErrorActionPreference = 'Stop'
$root = 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP'
$poolPath = "$root\reqpool\requirements-pool.json"
$pool = Get-Content $poolPath -Raw -Encoding UTF8 | ConvertFrom-Json
$now = (Get-Date).ToString('yyyy-MM-ddTHH:mm:ss')
foreach ($it in $pool.items) {
    if ($it.id -in @('R-20261116-001','R-20261116-002')) {
        $it.status = '完成发布'
        $it | Add-Member -NotePropertyName releaseVersion -NotePropertyValue $Version -Force
        $it.statusHistory = @($it.statusHistory) + @(@{
            status = '完成发布'; at = $now; note = "随 v$Version 发布(用户指示提前触发发布周期);GitHub/Gitee Release 与官网 downloads 五件套已上线"
        })
    }
}
$pool | ConvertTo-Json -Depth 12 | Set-Content $poolPath -Encoding UTF8
# 写后读回复核
$chk = Get-Content $poolPath -Raw -Encoding UTF8 | ConvertFrom-Json
$done = @($chk.items | Where-Object { $_.releaseVersion -eq $Version })
$pending = @($chk.items | Where-Object { $_.status -eq '记录' -and -not $_.PSObject.Properties['releaseVersion'] })
Write-Host "✅ 回写完成: $($done.Count) 条完成发布(v$Version);剩余待开发 $($pending.Count) 条"
