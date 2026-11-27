# apply-review.ps1 —— 应用 dsh 会话的 AI 复审结论(EasyRDP 反馈)
# 用法: powershell -File scripts\apply-review.ps1 -Verdicts <verdicts.json>
# verdicts.json: [ { "fbId": "...", "verdict": "real|spam|reject", "note": "判断理由(可选)" } ]
#   real  → 池条目保持「记录」,官网保持公开(已回复"已收录")
#   spam  → 池条目移除,官网下架(清回复、不公开)
#   reject→ 池条目置「已关闭」(note 说明拒绝原因),官网回复"该反馈暂不纳入开发计划"并下架
# 同时从 review-queue.json 移除已判定条目,写 review.log。
param(
    [Parameter(Mandatory=$true)][string]$Verdicts,
    [string]$PoolPath = '',
    [string]$QueuePath = '',
    [string]$LogPath = '',
    [string]$Server = 'root@easyrdp.cn',
    [int]$Port = 22,
    [string]$Key = 'P:\Program Files (x86)\EasySSH\keys\id_ed25519_easyssh'
)
$ScriptRoot = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
if (-not $PoolPath) { $PoolPath = Join-Path $ScriptRoot '..\reqpool\requirements-pool.json' }
if (-not $QueuePath) { $QueuePath = Join-Path $ScriptRoot '..\reqpool\review-queue.json' }
if (-not $LogPath) { $LogPath = Join-Path $ScriptRoot '..\reqpool\review.log' }
$ErrorActionPreference = 'Stop'
function Log($m) { Add-Content $LogPath ("- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') $m") -Encoding UTF8 }

$vd = Get-Content $Verdicts -Raw -Encoding UTF8 | ConvertFrom-Json
$pool = Get-Content $PoolPath -Raw -Encoding UTF8 | ConvertFrom-Json
$queue = @()
if (Test-Path $QueuePath) { $queue = @(Get-Content $QueuePath -Raw -Encoding UTF8 | ConvertFrom-Json) }
$applied = 0
$siteUpdates = @{}   # fbId → 'unpublish-replied' | 'unpublish-silent' | 'keep'

foreach ($v in @($vd)) {
    $fbId = [string]$v.fbId
    $ver = [string]$v.verdict
    $poolItem = @($pool.items | Where-Object { $_.sourceRef -eq $fbId }) | Select-Object -First 1
    switch ($ver) {
        'real' {
            $siteUpdates[$fbId] = 'keep'
            Log "real: 反馈 $fbId 保持入池公开$(if ($v.note) { " — $($v.note)" })"
            $applied++
        }
        'spam' {
            if ($poolItem) {
                $pool.items = @($pool.items | Where-Object { $_.id -ne $poolItem.id })
                Log "spam: 移除池条目 $($poolItem.id)(反馈 $fbId)$(if ($v.note) { " — $($v.note)" })"
            } else { Log "spam: 反馈 $fbId 无池条目(可能采集时已被垃圾过滤)" }
            $siteUpdates[$fbId] = 'unpublish-silent'
            $applied++
        }
        'reject' {
            if ($poolItem) {
                $now = (Get-Date).ToString('yyyy-MM-ddTHH:mm:ss')
                $poolItem.status = '已关闭'
                $poolItem.statusHistory = @($poolItem.statusHistory) + @(@{
                    status = '已关闭'; at = $now
                    note = "AI 复审判定不宜纳入开发$(if ($v.note) { ": $($v.note)" })"
                })
                Log "reject: 池条目 $($poolItem.id) 置「已关闭」(反馈 $fbId)$(if ($v.note) { " — $($v.note)" })"
            } else { Log "reject: 反馈 $fbId 无池条目" }
            $siteUpdates[$fbId] = 'unpublish-replied'
            $applied++
        }
        default { Log "未知 verdict: $ver(反馈 $fbId),跳过" }
    }
}

# 写回池(保持口径一致;写后读回复核)
$pool | ConvertTo-Json -Depth 12 | Set-Content $PoolPath -Encoding UTF8
$chk = Get-Content $PoolPath -Raw -Encoding UTF8 | ConvertFrom-Json
$pending = @($chk.items | Where-Object { $_.status -eq '记录' -and -not $_.PSObject.Properties['releaseVersion'] })
Log "池回写复核成功;池共 $($chk.items.Count) 条,待开发 $($pending.Count) 条"

# 官网侧下架/改回复(仅需变化的条目)
if ($siteUpdates.Count -gt 0) {
    $replyScript = Join-Path $env:TEMP ("rdp-review-apply-{0}.php" -f (Get-Date -Format 'yyyyMMddHHmmss'))
    $pairs = @()
    foreach ($k in $siteUpdates.Keys) { $pairs += "$k=$($siteUpdates[$k])" }
    @'
<?php
$f = "/www/wwwroot/easyrdp-data/feedback-db.json";
$db = json_decode((string)file_get_contents($f), true);
$ops = array_flip(explode(",", "__OPS__"));
$now = (int)(microtime(true) * 1000);
$upd = 0;
foreach ($db["items"] as $i => $it) {
    $id = $it["id"] ?? "";
    if (!isset($ops["$id=keep"])) continue;
    $op = $ops["$id=keep"] ?? "";
    if ($op === "unpublish-silent") {
        $db["items"][$i]["reply"] = "";
        $db["items"][$i]["replyAt"] = 0;
        $db["items"][$i]["public"] = false;
        $db["items"][$i]["status"] = "spam";
        $upd++;
    } elseif ($op === "unpublish-replied") {
        $db["items"][$i]["reply"] = "感谢您的反馈!经评估,该内容暂不纳入开发计划,已停止公开展示。";
        $db["items"][$i]["replyAt"] = $now;
        $db["items"][$i]["public"] = false;
        $upd++;
    }
}
file_put_contents($f, json_encode($db, JSON_PRETTY_PRINT | JSON_UNESCAPED_UNICODE));
echo "updated=$upd\n";
'@ -Replace '__OPS__', ($pairs -join ',') | Set-Content $replyScript -Encoding UTF8
    scp -q -i $Key -P $Port $replyScript "${Server}:/tmp/review-apply.php"
    $updLog = ssh -i $Key -p $Port -o BatchMode=yes -o StrictHostKeyChecking=accept-new $Server 'php /tmp/review-apply.php; rm -f /tmp/review-apply.php'
    Remove-Item $replyScript -Force -ErrorAction SilentlyContinue
    Log "官网状态同步: $updLog"
}

# 从队列移除已判定条目
$doneIds = @(@($vd) | ForEach-Object { [string]$_.fbId })
$queue = @($queue | Where-Object { $doneIds -notcontains [string]$_.fbId })
@($queue) | ConvertTo-Json -Depth 6 | Set-Content $QueuePath -Encoding UTF8
Log "复审完成:应用 $applied 条,队列剩余 $($queue.Count) 条"

# 回执邮件:复审判定落库后,给留了邮箱的用户发送处理通知(2026-11-26 与 EasySSH 对齐)
try {
    & (Join-Path $ScriptRoot 'send-feedback-receipts.ps1') -Server $Server -Port $Port -Key $Key
    Log "回执邮件流程执行完成"
} catch { Log "回执邮件流程异常(不影响复审): $($_.Exception.Message)" }

Write-Host "✅ 复审结论已应用: $applied 条;队列剩余 $($queue.Count) 条"
exit 0
