# ============================================================================
# EasyRDP 开发机角色一键安装脚本（Windows 10/11）
# ----------------------------------------------------------------------------
# 用途：完整复刻 EasyRDP 开发机——项目源码 + 构建依赖（vcpkg/FreeRDP/pkgconf）
#       + git/Node/Rust/VS BuildTools/WebView2/WSL2 + DSH 开发角色
#       + 定时任务（反馈采集 08:00/20:00、发布巡检 04:00）
# 用法：管理员 PowerShell 中执行
#       powershell -ExecutionPolicy Bypass -File install-windows.ps1
#                  [-ProjectDir B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP]
#                  [-ToolsDir B:\tools\EasyRDP] [-SkipDSH] [-SkipWsl] [-SkipScheduledTasks]
# 幂等  ：重复执行安全（已存在即跳过）。
# ============================================================================
param(
  [string]$ProjectDir = 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP',
  [string]$ToolsDir   = 'B:\tools\EasyRDP',
  [string]$DshDir     = 'P:\Program Files (x86)\DeepSeek Harness',
  [switch]$SkipDSH,
  [switch]$SkipWsl,
  [switch]$SkipScheduledTasks
)
$ErrorActionPreference = 'Continue'
$Pkg = Split-Path -Parent $MyInvocation.MyCommand.Path
$IsAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
           ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $IsAdmin) { Write-Warning '建议以管理员身份运行（部分安装需要）；继续执行...' }

function Have($cmd) { [bool](Get-Command $cmd -ErrorAction SilentlyContinue) }
function Step($m) { Write-Host "`n==> $m" -ForegroundColor Cyan }

# ---------- [1/10] 部署项目源码（EasyRDP 源码目录 → ProjectDir，robocopy 增量） ----------
Step '[1/10] 部署项目源码（robocopy 增量）'
if (-not (Test-Path "$Pkg\EasyRDP")) { throw "包内缺少 EasyRDP 源码目录" }
New-Item $ProjectDir -ItemType Directory -Force | Out-Null
robocopy "$Pkg\EasyRDP" "$ProjectDir" /E /NFL /NDL /NJH /NP /R:1 /W:1 | Out-Null
if ($LASTEXITCODE -ge 8) { throw "源码部署失败 ($LASTEXITCODE)" }
Write-Host "  已部署 → $ProjectDir"

# ---------- [2/10] 部署构建依赖工具链（vcpkg 快照等） ----------
Step '[2/10] 部署构建依赖 tools\EasyRDP → ToolsDir（robocopy 增量）'
if (Test-Path "$Pkg\tools\EasyRDP") {
  New-Item (Split-Path -Parent $ToolsDir) -ItemType Directory -Force | Out-Null
  robocopy "$Pkg\tools\EasyRDP" "$ToolsDir" /E /NFL /NDL /NJH /NP /R:1 /W:1 | Out-Null
  if ($LASTEXITCODE -ge 8) { throw "工具链复制失败 ($LASTEXITCODE)" }
  Write-Host "  已部署 → $ToolsDir"
} else { Write-Warning '包内缺少 tools\EasyRDP（vcpkg/FreeRDP 快照）——首次构建 FreeRDP 前必须补齐' }

# ---------- [3/10] 基础工具：git / Node.js LTS ----------
Step '[3/10] 安装 git / Node.js LTS（winget）'
if (-not (Have git))  { winget install --id Git.Git -e --accept-source-agreements --accept-package-agreements }
if (-not (Have node)) { winget install --id OpenJS.NodeJS.LTS -e --accept-source-agreements --accept-package-agreements }

# ---------- [4/10] Rust 工具链 ----------
Step '[4/10] 安装 Rust（rustup，stable-x86_64-pc-windows-msvc）'
if (-not (Have rustup)) {
  winget install --id Rustlang.Rustup -e --accept-source-agreements --accept-package-agreements
}
if (Have rustup) {
  rustup set default-host x86_64-pc-windows-msvc 2>$null
  rustup default stable-x86_64-pc-windows-msvc 2>$null
  rustup update 2>$null
} else {
  Write-Warning 'rustup 不可用：请手动安装 https://rustup.rs（选 MSVC）后重跑本脚本'
}

# ---------- [5/10] Visual Studio 2022 Build Tools ----------
Step '[5/10] 安装 VS2022 Build Tools（C++ 桌面开发，体积大需等待）'
$vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$hasCpp = $false
if (Test-Path $vsWhere) {
  $hasCpp = (& $vsWhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath) -ne $null
}
if (-not $hasCpp) {
  winget install --id Microsoft.VisualStudio.2022.BuildTools -e --accept-source-agreements --accept-package-agreements `
    --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
} else { Write-Host '  已具备 MSVC C++ 工具集，跳过' }

# ---------- [6/10] WebView2 Runtime ----------
Step '[6/10] 安装 WebView2 Runtime（Tauri 运行需要）'
winget install --id Microsoft.EdgeWebView2Runtime -e --accept-source-agreements --accept-package-agreements 2>$null

# ---------- [7/10] WSL2 + Ubuntu ----------
Step '[7/10] WSL2（release.sh 一键发布依赖；-SkipWsl 可跳过）'
if (-not $SkipWsl) {
  if (-not (Have wsl)) { wsl --install --no-launch } else { Write-Host '  WSL 已安装' }
  Write-Host '  如 Ubuntu 尚未安装：wsl --install -d Ubuntu（完成后需重启并设 UNIX 用户）'
}

# ---------- [8/10] DeepSeek Harness（DSH 开发角色） ----------
Step '[8/10] DeepSeek Harness（DSH）安装'
if ($SkipDSH) { Write-Host '  已跳过（-SkipDSH）' }
else {
  $dshExisting = @()
  foreach ($d in @($DshDir, "${env:ProgramFiles}\DeepSeek Harness", "${env:ProgramFiles(x86)}\DeepSeek Harness",
                   "$env:LOCALAPPDATA\Programs\DeepSeek Harness", "$env:LOCALAPPDATA\DeepSeek Harness")) {
    if (Test-Path $d) { $dshExisting += $d }
  }
  if ($dshExisting.Count -gt 0) {
    Write-Host "  已存在 DSH 安装：$($dshExisting -join ' ; ')"
  } else {
    $found = winget search 'DeepSeek Harness' --accept-source-agreements 2>$null | Select-String 'DeepSeek'
    if ($found) {
      $id = ($found[0] -split '\s{2,}')[0].Trim()
      winget install --id $id -e --accept-source-agreements --accept-package-agreements
    } else {
      Write-Warning @"
未在 winget 目录找到 DeepSeek Harness。DSH 开发角色需手动安装（三选一）：
  1) 从 DSH 官方发布渠道（官网/GitHub Releases）下载 exe/msi 安装包安装；
  2) 将原开发机的安装目录整体复制到：$DshDir
     （原机路径：P:\Program Files (x86)\DeepSeek Harness，含 app.asar 与 profile 数据）；
  3) 已有他机 DSH 时可整盘克隆后仅保留 DSH 目录。
  安装完成后：启动 DSH → Web GUI 打开 → 工作目录设为 $ProjectDir
  → 重建「提示型」定时任务（发布巡检 04:00 / 反馈采集复审）。
"@
    }
  }
}

# ---------- [9/10] 定时任务 ----------
Step '[9/10] 注册 Windows 定时任务（反馈采集 08:00/20:00 + 发布巡检 04:00）'
if (-not $SkipScheduledTasks) {
  $act1 = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument `
    "-NoProfile -ExecutionPolicy Bypass -File `"$ProjectDir\scripts\harvest-feedback.ps1`""
  $t1 = New-ScheduledTaskTrigger -Daily -At '08:00'
  $t2 = New-ScheduledTaskTrigger -Daily -At '20:00'
  Register-ScheduledTask -TaskName 'EasyRDP-FeedbackHarvest' -Action $act1 -Trigger $t1,$t2 `
    -RunLevel Highest -Force | Out-Null
  $act2 = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument `
    "-NoProfile -ExecutionPolicy Bypass -File `"$ProjectDir\scripts\release-check.ps1`""
  $t3 = New-ScheduledTaskTrigger -Daily -At '04:00'
  Register-ScheduledTask -TaskName 'EasyRDP-ReleaseCheck' -Action $act2 -Trigger $t3 `
    -RunLevel Highest -Force | Out-Null
  Write-Host '  ✓ EasyRDP-FeedbackHarvest（每日 08:00/20:00）'
  Write-Host '  ✓ EasyRDP-ReleaseCheck（每日 04:00）'
  Write-Host '  说明：原 DSH 内的定时任务为「提示型」，此处为直接执行型等价任务。'
}

# ---------- [10/10] 构建环境变量样板 + 自检提示 ----------
Step '[10/10] 生成构建环境变量样板 easyrdp-build-env.ps1'
@"
# easyrdp-build-env.ps1 —— 每次构建前 dot-source：. .\easyrdp-build-env.ps1
`$env:PKG_CONFIG_PATH = '$ToolsDir\vcpkg\installed\x64-windows-static-md\lib\pkgconfig'
`$env:PATH = '$ToolsDir\pkgconf-msi\PFiles64\pkgconf-3.0.10;' + `$env:PATH
Write-Host 'EasyRDP 构建环境已就绪'
"@ | Set-Content -Encoding UTF8 (Join-Path $ProjectDir 'easyrdp-build-env.ps1')

Write-Host ''
Write-Host '✅ 开发机角色安装完成' -ForegroundColor Green
Write-Host "  项目：$ProjectDir"
Write-Host "  依赖：$ToolsDir"
Write-Host "  自检：pwsh 中执行  . $ProjectDir\easyrdp-build-env.ps1 ; cd $ProjectDir\easyrdp-client ; cargo check -p easyrdp-rdp --features native"
Write-Host "  发布：wsl bash -c `"cd <项目路径> && bash scripts/release.sh <版本号>`""
Write-Host '  注意：secrets-vault 已随源码包部署（加密形态）；官网服务器 SSH 私钥不在本包。'
