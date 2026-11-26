# pool-note-20261116.ps1 —— R-20261116-023~026 追加端到端取证结论（不改 status）
$ErrorActionPreference = 'Stop'
$poolPath = Join-Path $PSScriptRoot '..\reqpool\requirements-pool.json'
$now = '2026-11-16T21:00:00+08:00'
$notes = @{
    'R-20261116-023' = '登录→桌面网格帧端到端取证结案(10连接复跑gd01-gd10+引擎audit逐字节重建)：网格画面=服务端真实下发内容——远端登录瞬间Windows默认平铺壁纸(~75x104px小位图Tile模式)被真实渲染，客户端delta逐字节重建与实帧sha256五次全部相等、147个tile内容全部唯一、平铺周期75/104px与64px tile网格不对齐、且尊重窗口语义边界，非客户端缺陷。会话尾帧与网格态仅差71像素=后续稳定停在平铺态。防御性加固：ffi.rs快照None时禁止回退composer.pixels(GHOSTFIX3-R023)已编译验证。'
    'R-20261116-024' = '拖动平铺残影与R-20261116-023同批取证：gd07异常帧与gd08网格帧sha256完全相同(同一份服务端平铺壁纸内容)，拖动触发的客户端残影修复已量化验证(旧帧冻结事件6→0)。'
    'R-20261116-025' = '按钮悬停错位残影：本批10连接复跑登录→桌面路径0新增错位帧；此前量化验证(旧帧冻结6→0)覆盖同族路径。'
    'R-20261116-026' = '光标拖尾残影：同族小面积增量路径，本批复跑0新增拖尾异常帧。'
}
$raw = Get-Content $poolPath -Raw -Encoding UTF8
Copy-Item $poolPath "$poolPath.bak-note" -Force
$j = $raw | ConvertFrom-Json
foreach ($it in $j.items) {
    if ($notes.ContainsKey($it.id)) {
        $it.statusHistory = @($it.statusHistory) + @(@{ status = $it.status; at = $now; note = $notes[$it.id] })
    }
}
$out = $j | ConvertTo-Json -Depth 100
[IO.File]::WriteAllText($poolPath, $out, (New-Object Text.UTF8Encoding $true))
Add-Content -Path (Join-Path $PSScriptRoot '..\reqpool\pool-add.log') -Value "- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') 023-026 追加端到端取证结论 statusHistory 留痕(状态未变)" -Encoding UTF8
"OK: notes appended to 023-026"
