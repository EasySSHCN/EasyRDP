# # 【版本号三级递进规则(v.1.2.3,2026-11-16 用户口径更新)】
#   1) 一级版本号(major,最高级):满足 10 次二级版本号更新才进行升级(minor 满 10 → major+1)
#   2) 二级版本号(minor,中级):满足 10 次三级版本号更新才进行升级(patch 满 10 → minor+1)
#   3) 三级版本号(patch):需求池每 3 个可开发的需求则推进一次三级版本升级;
#      或用户在会话中指定专门要求发布版本升级
#      (本脚本「待开发记录 >= 3 触发发布」即规则 3 的自动体现)
# release-check.ps1 —— EasyRDP 需求池每日巡检(12:00,定时任务触发)
# 规则1:池中状态=记录 的条目数 >= 3 → 触发一次发布周期:
#   - 计算下一版本号(next-version.ps1;patch+1;patch 满 10 升 minor;minor 满 10 升 major)
#   - 写出发布请求标志(release-request.json),由 dsh 会话执行开发/构建/发布
#   - 邮件通知 ad@6xm.net(经 notify-email.ps1;通道未配置时进 outbox)
# 规则2【里程碑,MILE-20261116 用户指定】:池累计需求记录每满 10 条 → 本次发布
#   升级一次二级版本号(minor+1、patch 归零,如 0.1.4 → 0.2.0);发布完成后由
#   发布会话把 meta.milestoneCount 更新为当时池条目总数,开始下一轮 10 条累计。
# 【版本号三级递进规则全文】见 scripts\next-version.ps1 头注释(1=major 满10次
#   minor更新升级;2=minor 满10次 patch 更新升级;3=patch 每10次推动 minor 升级)。
param(
    [string]$PoolPath = (Join-Path $PSScriptRoot '..\reqpool\requirements-pool.json'),
    [string]$FlagPath = (Join-Path $PSScriptRoot '..\reqpool\release-request.json'),
    [string]$LogPath = (Join-Path $PSScriptRoot '..\reqpool\release-check.log'),
    [string]$RepoRoot = 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP'
)
$ErrorActionPreference = 'Stop'
function Log($msg) { Add-Content -Path $LogPath -Value ("- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') $msg") -Encoding UTF8 }
try {
    $pool = Get-Content $PoolPath -Raw -Encoding UTF8 | ConvertFrom-Json
    # 需求池是唯一权威数据源:只认 requirements-pool.json 的当前 status 字段。
    # 防过期护栏:status=记录 但已带 releaseVersion 的条目属状态未同步的脏数据,一律排除并告警,
    # 避免再次把已完成条目误当待开发触发发布请求。
    $stale = @($pool.items | Where-Object { $_.status -eq '记录' -and $_.PSObject.Properties['releaseVersion'] })
    foreach ($s in $stale) { Log "告警:条目 $($s.id) 状态为「记录」但已带 releaseVersion=$($s.releaseVersion),疑为状态未同步,已排除" }
    $pending = @($pool.items | Where-Object { $_.status -eq '记录' -and -not $_.PSObject.Properties['releaseVersion'] })
    Log "巡检:待开发记录 $($pending.Count) 条($(if ($pending.Count) { ($pending | ForEach-Object { $_.id }) -join ', ' } else { '无' }))"

    # 已有未处理的发布请求则不重复写;已作废(voided)的历史请求归档留档,不阻塞下一次触发
    if (Test-Path $FlagPath) {
        $flag = Get-Content $FlagPath -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($flag.status -eq 'voided') {
            $archive = Join-Path (Split-Path $FlagPath) ("release-request-voided-{0}.json" -f (Get-Date -Format 'yyyyMMdd-HHmmss'))
            Move-Item $FlagPath $archive
            Log "历史请求已作废,归档至 $(Split-Path $archive -Leaf),继续本次巡检"
        } else { Log '已有未处理发布请求,跳过'; exit 0 }
    }

    if ($pending.Count -ge 3) {
        # EasyRDP 版本号唯一来源:easyrdp-client\src-tauri\tauri.conf.json
        $cur = (Get-Content (Join-Path $RepoRoot 'easyrdp-client\src-tauri\tauri.conf.json') -Raw -Encoding UTF8 | ConvertFrom-Json).version
        # —— 里程碑规则(MILE-20261116 用户指定):池累计需求记录每满 10 条 →
        #    本次发布升中级版本号(minor+1、patch 归零)。
        #    meta.milestoneCount = 上次里程碑升级时的池条目总数(首次为 0);
        #    发布完成后由发布会话把它更新为当时条目数,开始下一轮累计。
        $baseCount = 0
        if ($pool.PSObject.Properties['meta'] -and $pool.meta.PSObject.Properties['milestoneCount']) {
            $baseCount = [int]$pool.meta.milestoneCount
        }
        $totalCount = @($pool.items).Count
        $milestone = (($totalCount - $baseCount) -ge 10)
        if ($milestone) {
            Log "里程碑触发: 池累计需求 $totalCount 条(基线 $baseCount,增量>=10) → 本次发布升中级版本号"
        }
        $nvArgs = @{ Current = $cur }
        if ($milestone) { $nvArgs.Minor = $true }
        $next = & (Join-Path $RepoRoot 'scripts\next-version.ps1') @nvArgs
        @{ createdAt = (Get-Date).ToString('s'); currentVersion = $cur; nextVersion = $next;
           milestone = $milestone; poolTotal = $totalCount; milestoneBase = $baseCount;
           pendingIds = @($pending | ForEach-Object { $_.id }); status = 'requested';
           note = $(if ($milestone) { "里程碑发布: 本次开发完成后版本定为 v$next;发布完成后须把池 meta.milestoneCount 更新为 $totalCount(开始下一轮 10 条累计)" } else { '发布时需同步三处版本号: easyrdp-client\src-tauri\tauri.conf.json + easyrdp-client\Cargo.toml + easyrdp-gateway\Cargo.toml;官网 SITE_VERSION 由 scripts\release.sh 第 8 步自动同步;README.md 加 ### v<ver> 小节;发布入口 bash scripts/release.sh <ver>(WSL bash)' }) } |
            ConvertTo-Json -Depth 5 | Set-Content $FlagPath -Encoding UTF8
        $idList = (@($pending | ForEach-Object { $_.id }) -join ', ')
        Log "达到 3 条待开发记录 → 生成发布请求 v$cur → v$next(条目: $idList$(if ($milestone) { '; 里程碑 minor 升级' }))"
        $msTag = if ($milestone) { '(里程碑:中级版本升级)' } else { '' }
        & (Join-Path $RepoRoot 'scripts\notify-email.ps1') `
            -Subject "[EasyRDP] 需求池触发发布周期 v$cur → v$next$msTag" `
            -Text ("EasyRDP 需求池待开发记录已达 3 条,触发发布周期。`r`n当前版本: v$cur`r`n计划版本: v$next$msTag`r`n条目: " + $idList + "`r`n`r`n由 dsh 会话执行开发与发布,完成后另行通知。")
        exit 0
    }
    Log '未达 3 条,不触发发布'
} catch {
    Log "巡检异常: $($_.Exception.Message)"
    exit 1
}
