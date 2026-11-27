# send-feedback-receipts.ps1 —— EasyRDP 反馈回执邮件(2026-11-26 与 EasySSH 对齐)
# 功能:拉取官网反馈库,对"已有官方回复 + 联系方式为邮箱 + 尚未发过回执"的反馈,
#       给留言用户发送回执邮件:留言内容 + 官方回复 + 公开状态 + 精选页链接。
# 去重:已发送 fbId 记录在 reqpool\mail-receipts.json,重复调用不会重复发信。
# 由 harvest-feedback.ps1(自动回复后)与 apply-review.ps1(复审判定后)自动调用;也可手动执行。
param(
    [string]$Server = 'root@easyrdp.cn',
    [int]$Port = 22,
    [string]$Key = 'P:\Program Files (x86)\EasySSH\keys\id_ed25519_easyssh',
    [string]$StatePath = '',
    [string]$LogPath = ''
)
$ScriptRoot = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
if (-not $StatePath) { $StatePath = Join-Path $ScriptRoot '..\reqpool\mail-receipts.json' }
if (-not $LogPath)   { $LogPath   = Join-Path $ScriptRoot '..\reqpool\harvest.log' }
$ErrorActionPreference = 'Stop'
function Log($m) { Add-Content -Path $LogPath -Value ("- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') $m") -Encoding UTF8 }

$emailRe = '^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}$'
$fbUrl = 'https://easyrdp.cn/download.html'

# ---- 拉取反馈库 ----
$tmpFile = Join-Path $env:TEMP ("rdp-receipt-db-{0}.json" -f (Get-Date -Format 'yyyyMMddHHmmss'))
ssh -i $Key -p $Port -o BatchMode=yes -o ConnectTimeout=20 -o StrictHostKeyChecking=accept-new `
    $Server 'cat /www/wwwroot/easyrdp-data/feedback-db.json' | Out-File $tmpFile -Encoding UTF8
$fb = Get-Content $tmpFile -Raw -Encoding UTF8 | ConvertFrom-Json
Remove-Item $tmpFile -Force -ErrorAction SilentlyContinue

# ---- 已发送记录 ----
$sent = @{}
if (Test-Path $StatePath) {
    try { foreach ($x in @(Get-Content $StatePath -Raw -Encoding UTF8 | ConvertFrom-Json)) { $sent[[string]$x] = $true } } catch {}
}

$mailed = 0; $failed = 0
foreach ($it in @($fb.items)) {
    $id = [string]$it.id
    $reply = ([string]$it.reply).Trim()
    $contact = ([string]$it.contact).Trim()
    if (-not $reply -or -not $contact -or $sent.ContainsKey($id)) { continue }
    if ($contact -notmatch $emailRe) { continue }

    $public = [bool]$it.public
    $pubTxt = if ($public) { '已在官网「使用反馈精选」公开展示' } else { '暂未公开(仅作内部记录,不会公开展示)' }
    $subject = 'EasyRDP 反馈处理通知:感谢您的反馈,官方已回复'
    $text = @"
感谢您通过 EasyRDP 提交使用反馈,以下为处理结果:

【您的留言】($([string]$it.date) · $([string]$it.type))
$([string]$it.content)

【官方回复】
$reply

【公开状态】$pubTxt

您可以在官网下载页的「使用反馈精选」查看已公开的反馈: $fbUrl
再次感谢对 EasyRDP 的支持!
"@
    $html = @"
<div style="font-family:-apple-system,'Segoe UI',Arial,sans-serif;max-width:640px;margin:0 auto;color:#1f2937;">
<h2 style="margin:0 0 12px;">感谢您向 EasyRDP 提交反馈</h2>
<p style="color:#6b7280;margin:0 0 16px;">您的留言已被官方处理,以下为详情:</p>
<div style="border:1px solid #e5e7eb;border-radius:8px;padding:14px 18px;margin:10px 0;background:#f9fafb;">
<p style="color:#6b7280;margin:0 0 6px;font-size:13px;">【您的留言】$([string]$it.date) · $([string]$it.type)</p>
<p style="white-space:pre-wrap;margin:0;">$([System.Net.WebUtility]::HtmlEncode([string]$it.content))</p></div>
<div style="border:1px solid #d1fae5;border-radius:8px;padding:14px 18px;margin:10px 0;background:#f0fdf4;">
<p style="color:#6b7280;margin:0 0 6px;font-size:13px;">【官方回复】</p>
<p style="white-space:pre-wrap;margin:0;">$([System.Net.WebUtility]::HtmlEncode($reply))</p></div>
<p style="margin:12px 0;font-size:14px;"><strong>公开状态:</strong>$pubTxt</p>
<p style="margin:12px 0;font-size:14px;">您可以在官网下载页的<a href="$fbUrl">「使用反馈精选」</a>查看已公开的反馈。</p>
<p style="color:#9ca3af;font-size:13px;">再次感谢对 EasyRDP 的支持!本邮件由系统自动发送,请勿直接回复。</p></div>
"@
    & (Join-Path $ScriptRoot 'notify-email.ps1') -Subject $subject -Text $text -Html $html -To $contact | Out-Null
    if ($LASTEXITCODE -eq 0) {
        $sent[$id] = $true; $mailed++
        Log "回执邮件已发: $id → $contact"
    } else {
        $failed++
        Log "回执邮件发送失败(已留 outbox 待补发): $id → $contact"
    }
}

@($sent.Keys) | ConvertTo-Json -Depth 3 | Set-Content $StatePath -Encoding UTF8
Log "回执邮件处理完成:本次发送 $mailed 条,失败 $failed 条,累计已发 $($sent.Count) 条"
Write-Host "回执邮件:发送 $mailed / 失败 $failed / 累计 $($sent.Count)"
exit 0
