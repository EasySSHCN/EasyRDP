# pool-note-gf45.ps1 —— 023-026 追加 GHOSTFIX4/5 根治结论（不改 status）
$ErrorActionPreference = 'Stop'
$poolPath = Join-Path $PSScriptRoot '..\reqpool\requirements-pool.json'
$now = '2026-11-16T22:40:00+08:00'
$note = 'GHOSTFIX4/5 根治（2026-11-16 晚真机裁决）：用户复测 v0.2.2 预发布构建载入/悬停/拖动/点击四场景方块残影依旧且更重——增量管线本身（tile 逐包抠像素→16ms 合并批→呈现层贴回）为残影载体，多级时序缝隙不可逐一封堵。修复：①GHOSTFIX4 增量管线默认停用（EASYRDP_ENABLE_DELTAS=1 显式启用），只走 EndPaint 门控+整帧双路校验和快照全帧通路，发射线程终点加最后闸门；②GHOSTFIX5 呈现层帧世代计数（FRAME_GEN/BACKING_GEN）封堵 WM_PAINT blit 旧背板通道；③撤销首刷抑制（载入冻结+全帧模式死锁）。验证：present-audit delta=0、全帧时间轴健康、载入/欢迎屏/桌面抽帧目检干净。修复版已部署 P:\Program Files (x86)\EasyRDP\EasyRDP-0.2.2-portable.exe（hash 6FC8BF24...C4AFA）待用户真机复测。'
$raw = Get-Content $poolPath -Raw -Encoding UTF8
Copy-Item $poolPath "$poolPath.bak-gf45" -Force
$j = $raw | ConvertFrom-Json
foreach ($it in $j.items) {
    if ($it.id -in @('R-20261116-023','R-20261116-024','R-20261116-025','R-20261116-026')) {
        $it.statusHistory = @($it.statusHistory) + @(@{ status = $it.status; at = $now; note = $note })
    }
}
[IO.File]::WriteAllText($poolPath, ($j | ConvertTo-Json -Depth 100), (New-Object Text.UTF8Encoding $true))
Add-Content -Path (Join-Path $PSScriptRoot '..\reqpool\pool-add.log') -Value "- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') 023-026 追加 GHOSTFIX4/5 根治结论留痕" -Encoding UTF8
"OK"
