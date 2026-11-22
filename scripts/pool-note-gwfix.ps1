# pool-note-gwfix.ps1 —— R-20261120-008/009/010 开发收尾状态备注（不改 status，仅追加 statusHistory 留痕）
$ErrorActionPreference='Stop'
$pool = Get-Content 'reqpool\requirements-pool.json' -Raw -Encoding UTF8 | ConvertFrom-Json
$notes = @{
  'R-20261120-008' = '开发收尾完成：[auth] allow_placeholder_login 总闸(config.rs)+AuthState 闸门(auth.rs)+main.rs 注入已入库，cargo test 75/75 全绿；剩余仅发布工序（生产 config 置 false + nginx 热修复层随版部署）'
  'R-20261120-009' = '开发收尾完成：网关新增 POST /admin/issue（X-Admin-Key，TTL 钳制 3600s，4 单测全过）；issue.php 签发链改走 /admin/issue 回环；gw-admin.html 用户服务台页面已产出（21.5KB，接口字段全对齐）；剩余仅发布工序（users.db 初始化 + 站点部署）'
  'R-20261120-010' = '开发收尾完成：manual/gateway.html 新增「云网关服务状态与获取凭据」小节+gw-admin.html 入口链接；faq.html 正文与 JSON-LD 双处口径改齐，旧表述零残留；diff 记录于 easyrdp-fix/05-docs-fix.md；剩余仅发布工序（官网资产随版上传）'
}
$changed=0
foreach($it in $pool.items){
  if($notes.ContainsKey($it.id)){
    $it.statusHistory += [pscustomobject]@{ status = $it.status; at = (Get-Date).ToString('yyyy-MM-ddTHH:mm:ss+08:00'); note = $notes[$it.id] }
    $changed++
  }
}
$out = ConvertTo-Json $pool -Depth 10
[IO.File]::WriteAllText((Join-Path (Get-Location) 'reqpool\requirements-pool.json'), $out, (New-Object Text.UTF8Encoding $false))
# 写后读回复核
$chk = Get-Content 'reqpool\requirements-pool.json' -Raw -Encoding UTF8 | ConvertFrom-Json
foreach($id in $notes.Keys){ $e = $chk.items | Where-Object {$_.id -eq $id}; $last = $e.statusHistory[-1].note; if(-not $last.StartsWith('开发收尾完成')){ throw "回读失败: $id" } }
Add-Content -Path 'reqpool\pool-add.log' -Value ("- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') pool-note-gwfix: $changed 条开发收尾备注写入并回读复核通过") -Encoding UTF8
"OK: $changed 条已备注并复核"