# harvest-feedback.ps1 —— EasyRDP 官网反馈采集入库(需求池)
# 每日 08:00 / 20:00 由 DSH 定时任务「EasyRDP 反馈采集与 AI 复审」调用。
# 功能:SSH 到 easyrdp.cn 服务器读取 /www/wwwroot/easyrdp-data/feedback-db.json,
#       垃圾启发式过滤后,把池中尚不存在的新反馈:
#       1) 分析登记入池(status=记录,source=官网反馈,sourceRef=反馈 id)
#       2) 写待审队列 reqpool\review-queue.json 供 AI 复审
#       3) 官网自动回复「已收录」并公开到前台 TOP10(spam 由复审阶段下架)
# 垃圾内容:不入池、不回复、不公开。
# 机密:池文件含用户反馈内容,禁止外泄/提交(.gitignore 已排除 reqpool\)。
param(
    [string]$Server = 'root@easyrdp.cn',
    [int]$Port = 22,
    [string]$Key = 'P:\Program Files (x86)\EasySSH\keys\id_ed25519_easyssh',
    [string]$PoolPath = (Join-Path $PSScriptRoot '..\reqpool\requirements-pool.json'),
    [string]$QueuePath = (Join-Path $PSScriptRoot '..\reqpool\review-queue.json'),
    [string]$LogPath = (Join-Path $PSScriptRoot '..\reqpool\harvest.log')
)
$ScriptRoot = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
if (-not $PoolPath) { $PoolPath = Join-Path $ScriptRoot '..\reqpool\requirements-pool.json' }
if (-not $QueuePath) { $QueuePath = Join-Path $ScriptRoot '..\reqpool\review-queue.json' }
if (-not $LogPath) { $LogPath = Join-Path $ScriptRoot '..\reqpool\harvest.log' }
$ErrorActionPreference = 'Stop'
function Log($msg) {
    Add-Content -Path $LogPath -Value ("- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') $msg") -Encoding UTF8
}
try {
    # ---- 0. 内容质量审核(垃圾/测试内容不入池、不回复、不公开) ----
    # 启发式:1) 非空白字符<6  2) 单字符连跑≥10  3) 任意6字符片段重复≥3次(复读灌水)
    #        4) 平台测试占位标记  5) 去空白后唯一字符占比<0.2(低信息熵)
    function Test-Junk([string]$c) {
        if (-not $c) { return $true }
        $t = ($c -replace '\s+', '')
        if ($t.Length -lt 6) { return $true }
        $run = 1
        for ($i = 1; $i -lt $t.Length; $i++) {
            if ($t[$i] -eq $t[$i-1]) { $run++; if ($run -ge 10) { return $true } } else { $run = 1 }
        }
        if ($t -match '可能含有违规信息') { return $true }
        $grams = @{}
        for ($i = 0; $i + 6 -le $t.Length; $i++) {
            $g = $t.Substring($i, 6)
            $grams[$g] = [int]$grams[$g] + 1
            if ($grams[$g] -ge 3) { return $true }
        }
        $uniq = ($t.ToCharArray() | Select-Object -Unique).Count
        if ($t.Length -ge 20 -and ($uniq / $t.Length) -lt 0.2) { return $true }
        return $false
    }

    # ---- 1. 拉取官网反馈库(数据目录在 webroot 外 /www/wwwroot/easyrdp-data/) ----
    $tmpFile = Join-Path $env:TEMP ("rdp-feedback-db-{0}.json" -f (Get-Date -Format 'yyyyMMddHHmmss'))
    ssh -i $Key -p $Port -o BatchMode=yes -o ConnectTimeout=20 -o StrictHostKeyChecking=accept-new `
        $Server 'cat /www/wwwroot/easyrdp-data/feedback-db.json' | Out-File $tmpFile -Encoding UTF8
    if (-not (Test-Path $tmpFile) -or (Get-Item $tmpFile).Length -eq 0) {
        Log '采集失败:远端反馈库为空或 SSH 失败'; Remove-Item $tmpFile -Force -ErrorAction SilentlyContinue; exit 1
    }
    $fb = Get-Content $tmpFile -Raw -Encoding UTF8 | ConvertFrom-Json
    Remove-Item $tmpFile -Force -ErrorAction SilentlyContinue
    $items = @($fb.items)
    Log "拉取远端反馈库成功:共 $($items.Count) 条"

    # ---- 2. 读池,找新反馈 ----
    $pool = Get-Content $PoolPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $known = @{}
    foreach ($p in $pool.items) { $known[[string]$p.sourceRef] = $true }
    $new = @($items | Where-Object { -not $known[[string]$_.id] })
    if (-not $new.Count) { Log '无新增反馈'; exit 0 }

    # ---- 3. 逐条处理 ----
    $queue = @()
    if (Test-Path $QueuePath) { $queue = @(Get-Content $QueuePath -Raw -Encoding UTF8 | ConvertFrom-Json) }
    $added = 0; $junked = 0
    foreach ($it in $new) {
        $id = [string]$it.id
        $content = [string]$it.content
        if (Test-Junk $content) {
            $junked++
            Log "垃圾过滤: $id「$($content.Substring(0, [Math]::Min(30, $content.Length)))」不入池不回复"
            continue
        }
        $today = Get-Date -Format 'yyyyMMdd'
        # 流水号:当日池内已有该日 id 数 +1
        $seq = 1 + @($pool.items | Where-Object { $_.id -match "^R-$today-" }).Count
        $poolId = "R-$today-{0:d3}" -f $seq
        $createdAt = if ($it.ts) { [DateTimeOffset]::FromUnixTimeMilliseconds([int64]$it.ts).ToLocalTime().ToString('yyyy-MM-ddTHH:mm:ss') } else { (Get-Date).ToString('yyyy-MM-ddTHH:mm:ss') }
        $ver = [string]$it.appVersion
        $title = $content.Substring(0, [Math]::Min(60, $content.Length)) + $(if ($content.Length -gt 60) { '…' } else { '' })
        $item = [ordered]@{
            id = $poolId
            type = if ([string]$it.type -eq '功能建议') { '需求' } else { '反馈' }
            title = "官网反馈[$([string]$it.type)] $title"
            source = '官网反馈'
            sourceRef = $id
            detail = "【用户反馈】$content`n【环境】版本=$ver 联系方式=$([string]$it.contact) 提交时间=$([string]$it.date)`n【系统信息】$(($it.systemInfo | ConvertTo-Json -Compress -Depth 3))`n【来源】easyrdp.cn 软件端反馈提交,垃圾启发式过滤通过"
            createdAt = $createdAt
            statusHistory = @(@{ status = '记录'; at = $createdAt; note = "官网反馈采集入库(反馈 id $id);待 AI 复审" })
            status = '记录'
        }
        $itemFile = Join-Path $ScriptRoot ("..\reqpool\pending-items\$poolId.json")
        $item | ConvertTo-Json -Depth 8 | Set-Content $itemFile -Encoding UTF8
        & (Join-Path $ScriptRoot 'pool-add.ps1') -ItemFile $itemFile | Out-Null
        if ($LASTEXITCODE -ne 0) { Log "登记失败: $poolId(反馈 $id),跳过"; continue }
        $added++
        $pool = Get-Content $PoolPath -Raw -Encoding UTF8 | ConvertFrom-Json
        $queue = @($queue) + @(@{ fbId = $id; poolId = $poolId; type = [string]$it.type; content = $content; date = [string]$it.date })
        Log "入池: $poolId ← 反馈 $id「$($title.Substring(0, [Math]::Min(30, $title.Length)))」"
    }

    # ---- 4. 写待审队列 ----
    @($queue) | ConvertTo-Json -Depth 6 | Set-Content $QueuePath -Encoding UTF8
    Log "采集完成:新增入池 $added 条,垃圾拦截 $junked 条,待审队列 $($queue.Count) 条"

    # ---- 5. 官网自动回复「已收录」并公开(仅对本次入池条目;spam 由复审阶段下架) ----
    if ($added -gt 0) {
        $replyScript = Join-Path $env:TEMP ("rdp-harvest-reply-{0}.php" -f (Get-Date -Format 'yyyyMMddHHmmss'))
        $ids = (@($queue) | ForEach-Object { $_.fbId }) -join "','"
        @'
<?php
$f = "/www/wwwroot/easyrdp-data/feedback-db.json";
$db = json_decode((string)file_get_contents($f), true);
$ids = array_flip(explode(",", "__IDS__"));
$now = (int)(microtime(true) * 1000);
$upd = 0;
foreach ($db["items"] as $i => $it) {
    if (isset($ids[$it["id"]]) && trim((string)($it["reply"] ?? "")) === "") {
        $db["items"][$i]["reply"] = "已收录您的反馈,感谢支持!我们会认真评估并在后续版本中考虑。";
        $db["items"][$i]["replyAt"] = $now;
        $db["items"][$i]["status"] = "replied";
        $db["items"][$i]["public"] = true;
        $upd++;
    }
}
file_put_contents($f, json_encode($db, JSON_PRETTY_PRINT | JSON_UNESCAPED_UNICODE));
echo "updated=$upd\n";
'@ -Replace '__IDS__', $ids | Set-Content $replyScript -Encoding UTF8
        scp -q -i $Key -P $Port $replyScript "${Server}:/tmp/harvest-reply.php"
        $updLog = ssh -i $Key -p $Port -o BatchMode=yes -o StrictHostKeyChecking=accept-new $Server 'php /tmp/harvest-reply.php; rm -f /tmp/harvest-reply.php'
        Remove-Item $replyScript -Force -ErrorAction SilentlyContinue
        Log "官网自动回复并公开: $updLog"
    }
    exit 0
} catch {
    Log "采集异常: $($_.Exception.Message)"
    exit 1
}
