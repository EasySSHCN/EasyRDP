# publish-writeback-0.2.1.ps1 —— v0.2.1 发布完成后池状态回写（发布会话专用）
param([string]$Version = '0.2.1')
$ErrorActionPreference = 'Stop'
$root = 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP'
$poolPath = "$root\reqpool\requirements-pool.json"
$pool = Get-Content $poolPath -Raw -Encoding UTF8 | ConvertFrom-Json
$now = (Get-Date).ToString('yyyy-MM-ddTHH:mm:ss')
$ids = @('R-20261116-015','R-20261116-018','R-20261116-019','R-20261116-020','R-20261116-021','R-20261116-022')
$notes = @{
  'R-20261116-015' = 'v0.1.5 载入/拖动残影调查条目:根因已随 v0.2.1 呈现层时序反转修复收口,完成发布'
  'R-20261116-018' = '边缘 resize+条纹错位修复随 v0.2.1 一并发布收口;载入/拖动遗留残影由呈现层时序反转修复根治'
  'R-20261116-019' = '随 v0.2.1 发布(此前已开发未发布);GitHub/Gitee Release 与官网 downloads 五件套+OTA 加密升级包已上线'
  'R-20261116-020' = '随 v0.2.1 发布;根因=呈现层 Delta/全帧时序反转,全帧前冲刷增量批恢复严格事件序,真机流量回放 8 残影帧归零'
  'R-20261116-021' = '随 v0.2.1 发布;同根因(拖动期旧增量覆盖新全帧),已根治待用户真机验证'
  'R-20261116-022' = '随 v0.2.1 发布;同根因(按钮高亮增量晚到闪回旧按钮态),已根治待用户真机验证'
}
foreach ($it in $pool.items) {
    if ($it.id -in $ids) {
        $it.status = '完成发布'
        $it | Add-Member -NotePropertyName releaseVersion -NotePropertyValue $Version -Force
        $it.statusHistory = @($it.statusHistory) + @(@{
            status = '完成发布'; at = $now; note = $notes[$it.id]
        })
    }
}
$pool | ConvertTo-Json -Depth 12 | Set-Content $poolPath -Encoding UTF8
# 写后读回复核
$chk = Get-Content $poolPath -Raw -Encoding UTF8 | ConvertFrom-Json
$done = @($chk.items | Where-Object { $_.releaseVersion -eq $Version })
$pending = @($chk.items | Where-Object { ($_.status -eq '记录' -or $_.status -eq '待开发') -and -not $_.PSObject.Properties['releaseVersion'] })
$stale = @($chk.items | Where-Object { $_.status -in @('记录','待开发') -and $_.PSObject.Properties['releaseVersion'] })
Write-Host "OK 回写完成: $($done.Count) 条完成发布(v$Version);剩余待处理 $($pending.Count) 条;防过期护栏异常 $($stale.Count) 条"
