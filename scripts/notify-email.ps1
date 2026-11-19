# notify-email.ps1 —— EasyRDP 发布通知邮件(发布完成后调用,收件人 ad@6xm.net)
# 发送通道(按优先级):
#   1) HTTP 邮件 API:设置环境变量 EASYRDP_MAIL_API_URL(POST JSON: {to,subject,text})
#      与 EASYRDP_MAIL_API_KEY(Bearer Token)
#   2) SMTP:设置 EASYRDP_SMTP_HOST / EASYRDP_SMTP_PORT / EASYRDP_SMTP_USER / EASYRDP_SMTP_PASS
#   3) 均未配置:优先复用本机 EasySSH 共享 DPAPI 加密凭据 mail-api.cred(同机同用户可解密);
#      仍无 → 写入待发邮箱 outbox,待配置后由 -FlushOutbox 补发(绝不静默丢失)
param(
    [string]$Subject,
    [string]$Text,
    [string]$To = 'ad@6xm.net',
    [switch]$FlushOutbox
)
$ErrorActionPreference = 'Stop'
$ScriptRoot = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
if ((-not $FlushOutbox) -and ((-not $Subject) -or (-not $Text))) {
    throw "缺少必填参数: -Subject 与 -Text(单独补发请用 -FlushOutbox)"
}
$Outbox = Join-Path $ScriptRoot '..\reqpool\outbox'
New-Item -ItemType Directory -Force $Outbox | Out-Null

function Save-Outbox($s, $t) {
    $f = Join-Path $Outbox ("pending-{0:yyyyMMdd-HHmmss}.json" -f (Get-Date))
    @{ to = $To; subject = $s; text = $t; createdAt = (Get-Date).ToString('s') } |
        ConvertTo-Json | Set-Content $f -Encoding UTF8
    Write-Host "⚠ 邮件通道未配置,已写入待发: $f"
}

function Send-Api($s, $t) {
    $url = $env:EASYRDP_MAIL_API_URL; $key = $env:EASYRDP_MAIL_API_KEY
    # 凭据优先级:本项目 reqpool\mail-api.cred → EasySSH 共享加密凭据(DPAPI,机密禁止外泄)
    foreach ($credFile in @(
        (Join-Path $ScriptRoot '..\reqpool\mail-api.cred'),
        'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasySSH\reqpool\mail-api.cred'
    )) {
        if (-not $url -and (Test-Path $credFile)) {
            try {
                $c = Get-Content $credFile -Raw -Encoding UTF8 | ConvertFrom-Json
                $url = $c.api
                $key = [Runtime.InteropServices.Marshal]::PtrToStringAuto([Runtime.InteropServices.Marshal]::SecureStringToBSTR((ConvertTo-SecureString $c.key)))
                break
            } catch { }
        }
    }
    if (-not $url -or -not $key) { return $false }
    if ($url -notmatch '/v1/send$') { $url = $url.TrimEnd('/') + '/v1/send' }
    try {
        # 显式 UTF-8 字节发送——PowerShell 5.1 默认按 ASCII 编码请求体,中文会变 ?
        $body = @{ to = @($To); subject = $s; text = $t } | ConvertTo-Json
        $utf8 = [System.Text.Encoding]::UTF8.GetBytes($body)
        Invoke-RestMethod -Uri $url -Method Post -ContentType 'application/json; charset=utf-8' `
            -Headers @{ Authorization = "Bearer $key" } -Body $utf8 -TimeoutSec 30 | Out-Null
        Write-Host "✅ 通知邮件已发送(API): $s"
        return $true
    } catch { Write-Host "✗ 邮件 API 发送失败: $($_.Exception.Message)"; return $false }
}

function Send-Smtp($s, $t) {
    if (-not $env:EASYRDP_SMTP_HOST) { return $false }
    try {
        $sec = ConvertTo-SecureString $env:EASYRDP_SMTP_PASS -AsPlainText -Force
        $cred = New-Object System.Management.Automation.PSCredential($env:EASYRDP_SMTP_USER, $sec)
        $smtpPort = 587
        if ($env:EASYRDP_SMTP_PORT) { $smtpPort = [int]$env:EASYRDP_SMTP_PORT }
        Send-MailMessage -SmtpServer $env:EASYRDP_SMTP_HOST -Port $smtpPort `
            -UseSsl -Credential $cred -From $env:EASYRDP_SMTP_USER -To $To `
            -Subject $s -Body $t -Encoding UTF8
        Write-Host "✅ 通知邮件已发送(SMTP): $s"
        return $true
    } catch { Write-Host "✗ SMTP 发送失败: $($_.Exception.Message)"; return $false }
}

if ($FlushOutbox) {
    Get-ChildItem $Outbox -Filter 'pending-*.json' | Sort-Object Name | ForEach-Object {
        $m = Get-Content $_.FullName -Raw -Encoding UTF8 | ConvertFrom-Json
        if ((Send-Api $m.subject $m.text) -or (Send-Smtp $m.subject $m.text)) { Remove-Item $_.FullName -Force }
        else { Write-Host "✗ 补发失败,保留待发: $($_.Name)" }
    }
    exit 0
}

if ((-not (Send-Api $Subject $Text)) -and (-not (Send-Smtp $Subject $Text))) {
    Save-Outbox $Subject $Text; exit 2   # 待发(outbox),不算硬失败
}
