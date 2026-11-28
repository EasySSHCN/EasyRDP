# publish-writeback-0.4.1.ps1 —— v0.4.1 发布完成后池状态回写（发布会话专用）
$ErrorActionPreference = 'Stop'
$root = 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP'
$poolPath = "$root\reqpool\requirements-pool.json"
$now = '2026-11-20T11:45:00+08:00'
$pool = Get-Content $poolPath -Raw -Encoding UTF8 | % { $_.TrimStart([char]0xFEFF) } | ConvertFrom-Json
$notes = @{
  'R-20261119-006' = '随 v0.4.1 发布：首启 PowerShell 黑窗修复核验通过（create_shortcut Command 链 CREATE_NO_WINDOW 0x08000000 + CommandExt，已在线上代码确认，cargo check 通过）'
  'R-20261119-007' = '随 v0.4.1 发布：官网下载链接 Token 化上线。api/download/{common,token,get,index}.php + token/ 路由（HMAC-SHA256 签名 + 滑动窗口 10 分钟 + IP 段绑定(/24，可关) + 同 token 16 并发上限 + 同 IP 每分钟频控 + 下载日志）；download.html 中英 30 链接全部 data-dl 化，页面静态内容零真实文件 URL；Range 断点续传/多线程下载器兼容；服务器部署 keys.php（密钥不出服务器）；修复 api/stats/common.php 与 feedback/submit.php BOM 污染 JSON 响应问题；线上端到端验证：token 签发 200/白名单外 403/无 Referer 放行/完整文件流 5.4MB 正常'
  'R-20261119-008' = '随 v0.4.1 发布：取消远程桌面窗口强制置顶。rdp_view_final.rs（attach/全屏切换两处）+ native_view.rs + nv_part2.rs 全部 HWND_TOPMOST→HWND_TOP 一次性提到顶，保留前台焦点抢取；普通窗口可覆盖，交互点击指定'
  'R-20261119-010' = '随 v0.4.1 发布：访客设备归因补全。api/stats/client.php 新增无票据上报的「同 IP 段（IPv4 前16位段/IPv6 /32）≤24h」回溯归因（origin{via:ip-segment, uv, ip, visitTs, confidence:low}，仅当日首报且档案无 origin 时写入，票据归因优先）；admin.html SRC_LABEL 增 ip-segment 标签与疑似标注'
  'R-20261119-011' = '随 v0.4.1 发布：admin.html「已产生设备的访客记录」#v-produced 去内层卡片容器改纯容器，单层化对齐「最近访客」分区；自查其它分区无同类嵌套'
  'R-20261119-012' = '随 v0.4.1 发布：官网遗留打点清理。compare.html/migrate.html 移除 visit-beacon.js 引用并删除该脚本文件（api/visit/track.php 全站零引用无需处理）；服务器端已 rm 线上残留文件；全站死链核对无其它死引用'
  'R-20261120-001' = '随 v0.4.1 发布：悬停说明层链接修复核验通过（tooltip.js bindTipHover 幂等绑定/层 mouseenter 清计时/mouseleave 300ms/按钮 blur 清计时器，既有实现确认无需改动）'
  'R-20261120-002' = '随 v0.4.1 发布：Git 同步「未检测到系统 git」报错增强。settings.js GIT_MISSING_HELP_URL + appendGitMissingHelp 内嵌 4 步处理说明（复用 open_manual_url 白名单直达），3 处挂接（状态区/push catch/pull catch）+ style.css 新样式；官网新增 manual/git-sync-no-git.html 帮助页并与 manual 总目录/features/download/faq 互链，线上 200'
}
foreach ($it in $pool.items) {
    if ($notes.ContainsKey($it.id)) {
        $it.status = '完成发布'
        $it | Add-Member -NotePropertyName releaseVersion -NotePropertyValue '0.4.1' -Force
        $it.statusHistory = @($it.statusHistory) + @(@{
            status = '完成发布'; at = $now
            note = $notes[$it.id]
        })
    }
}
# 里程碑基线：发布完成时 milestoneCount = 当时池条目总数（MILE 规则）
$total = $pool.items.Count
$pool.meta | Add-Member -NotePropertyName milestoneCount -NotePropertyValue $total -Force
Copy-Item $poolPath "$poolPath.bak-041" -Force
[IO.File]::WriteAllText($poolPath, ($pool | ConvertTo-Json -Depth 100), (New-Object Text.UTF8Encoding $true))
Remove-Item "$root\reqpool\release-request.json" -Force
Add-Content -Path "$root\reqpool\pool-add.log" -Value "- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') v0.4.1 发布回写：8 条(R-20261119-006/007/008/010/011/012, R-20261120-001/002)完成发布；milestoneCount=$total；release-request.json 已删除" -Encoding UTF8
$chk = Get-Content $poolPath -Raw -Encoding UTF8 | % { $_.TrimStart([char]0xFEFF) } | ConvertFrom-Json
$rest = @($chk.items | Where-Object { $_.status -in '记录','待开发','开发中' })
"回写完成: 池总数 $($chk.items.Count); 非终态 $($rest.Count) 条($((($rest | ForEach-Object id) -join ','))); milestoneCount=$($chk.meta.milestoneCount); release-request 存在=$(Test-Path "$root\reqpool\release-request.json")"
