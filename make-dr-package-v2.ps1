# ============================================================================
# EasyRDP 灾备包重建脚本 v2（2026-11-18）
# ----------------------------------------------------------------------------
# 删除旧灾备包 staging，按「开发机 / 远程服务器」双角色目录全新重建，
# 最终产出 zip：default-workspace\dr-build\EasyRDP-灾备包-YYYYMMDD.zip
# 用法：pwsh -File make-dr-package-v2.ps1 [-KeepStage]
# ============================================================================
param([switch]$KeepStage)
$ErrorActionPreference = 'Stop'
$WS    = 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace'
$PRJ   = Join-Path $WS 'EasyRDP'
$TOOL  = 'B:\tools\EasyRDP'
$OLD   = Join-Path $PRJ 'dr-build'          # 旧 staging（重建前短暂保留以取服务器快照）
$OLDH  = Join-Path $PRJ 'dr-build-old-hold' # 旧 staging 暂存名
$STAGE = Join-Path $PRJ 'dr-build'          # 新 staging（重建后复用原名）
$ASSETS= Join-Path $PRJ 'dr-assets'
$OUTDIR= Join-Path $WS 'dr-build'
$DATE  = Get-Date -Format 'yyyyMMdd'
$ZIP   = Join-Path $OUTDIR ("EasyRDP-灾备包-$DATE.zip")

function Copy-Tree([string]$src, [string]$dst, [string[]]$xd) {
    $a = @($src, $dst, '/E', '/NFL', '/NDL', '/NJH', '/NP', '/R:1', '/W:1')
    foreach ($d in $xd) { $a += @('/XD', $d) }
    robocopy @a | Out-Null
    if ($LASTEXITCODE -ge 8) { throw "robocopy failed ($LASTEXITCODE): $src -> $dst" }
}
function Step($m) { Write-Host "`n==> $m" -ForegroundColor Cyan }

# ---- [1] 旧包暂存处理（服务器快照 webroot/keys/data 仅存在于旧 staging） ----
Step '[1/8] 旧灾备包暂存处理'
if ((Test-Path $OLDH) -and (Test-Path "$OLDH\remote-server\webroot")) {
    # 快照已在暂存处；当前 dr-build 为上次失败的半成品，直接删除
    if (Test-Path $OLD) { Remove-Item $OLD -Recurse -Force }
} elseif (Test-Path $OLD) {
    if (Test-Path $OLDH) { Remove-Item $OLDH -Recurse -Force }
    Rename-Item $OLD $OLDH
} else {
    throw '未找到旧灾备包快照（dr-build/dr-build-old-hold 均缺 remote-server\webroot）'
}
$SnapRoot = Join-Path $OLDH 'remote-server'
if (-not (Test-Path "$SnapRoot\webroot")) { throw '旧 staging 缺少 remote-server\webroot 快照，无法重建' }

# ---- [2] 新 staging 骨架 ----
Step '[2/8] 创建新 staging 骨架'
New-Item "$STAGE\dev-machine", "$STAGE\remote-server" -ItemType Directory -Force | Out-Null

# ---- [3] 开发机：项目全量源码目录（含 secrets-vault/reqpool/dist，排除构建目录）----
# 注：bsdtar 在本项目巨树上会段错误崩溃，故用 robocopy 目录形态打包（安装脚本亦按目录部署）
Step '[3/8] 复制项目全量源码目录 EasyRDP（robocopy 增量）'
Copy-Tree "$WS\EasyRDP" "$STAGE\dev-machine\EasyRDP" @('target', 'node_modules', 'dr-build', 'dr-build-old-hold', 'dr-pull-work', 'sv-standby', 'dr-assets')
# 排除根目录的历史源码归档（体积大且非必需）
Get-ChildItem "$STAGE\dev-machine\EasyRDP" -Filter 'EasyRDP-src-*.tar.gz' | Remove-Item -Force

# ---- [4] 开发机：构建依赖工具链（vcpkg 快照等，来自 B:\tools\EasyRDP）----
Step '[4/8] 复制构建依赖工具链 tools\EasyRDP（约数 GB，需等待）'
$TD = "$STAGE\dev-machine\tools\EasyRDP"
New-Item "$TD\vcpkg" -ItemType Directory -Force | Out-Null
Copy-Tree "$TOOL\vcpkg\installed"              "$TD\vcpkg\installed" @()
Copy-Tree "$TOOL\vcpkg\buildtrees\freerdp\src" "$TD\vcpkg\buildtrees\freerdp\src" @()
Copy-Tree "$TOOL\vcpkg\ports"                  "$TD\vcpkg\ports" @()
Copy-Tree "$TOOL\vcpkg\scripts"                "$TD\vcpkg\scripts" @()
foreach ($f in 'vcpkg.exe','bootstrap-vcpkg.bat') {
  if (Test-Path "$TOOL\vcpkg\$f") { Copy-Item "$TOOL\vcpkg\$f" "$TD\vcpkg\$f" }
}
foreach ($f in '.vcpkg-root','.gitignore') {
  if (Test-Path "$TOOL\vcpkg\$f") { Copy-Item -Force "$TOOL\vcpkg\$f" "$TD\vcpkg\$f" }
}
Copy-Tree "$TOOL\pkgconf-msi"        "$TD\pkgconf-msi" @()
Copy-Tree "$TOOL\overlay-ports"      "$TD\overlay-ports" @()
Copy-Tree "$TOOL\freerdp2-sys-0.2.0" "$TD\freerdp2-sys-0.2.0" @()
foreach ($f in 'f2sys.crate','baseline.txt','freerdp-versions.json','pkgconf.msi') {
  if (Test-Path "$TOOL\$f") { Copy-Item "$TOOL\$f" "$TD\$f" }
}

# ---- [5] 开发机：一键安装脚本 + 恢复指南 ----
Step '[5/8] 写入开发机安装脚本与文档'
Copy-Item "$ASSETS\install-windows.ps1" "$STAGE\dev-machine\install-windows.ps1" -Force
Copy-Item "$ASSETS\install-linux.sh"    "$STAGE\dev-machine\install-linux.sh"    -Force
Copy-Item "$ASSETS\RESTORE-DEV.md"      "$STAGE\dev-machine\RESTORE-DEV.md"      -Force
# CRLF→LF（保证 bash 可执行）
$lf = Get-Content "$ASSETS\install-linux.sh" -Raw
[System.IO.File]::WriteAllText("$STAGE\dev-machine\install-linux.sh", ($lf -replace "`r`n","`n"))

# ---- [6] 远程服务器：站点/数据快照 + 机密 + 系统配置 + 脚本 ----
Step '[6/8] 复制远程服务器角色（站点/数据/机密/系统配置）'
# 站点全源：webroot 快照以目录形态进入包内，顶层目录名 easyrdp.cn
Copy-Tree "$SnapRoot\webroot" "$STAGE\remote-server\wwwroot\easyrdp.cn" @()
# 反馈数据目录
Copy-Tree "$SnapRoot\easyrdp-data" "$STAGE\remote-server\easyrdp-data" @()

# 明文机密 keys.php（webroot 内真实版）
Copy-Item "$SnapRoot\webroot\api\feedback\keys.php" "$STAGE\remote-server\keys.php" -Force

# 系统配置快照（nginx + php-fpm）
if (Test-Path "$OLDH\sys-config") {
  Copy-Tree "$OLDH\sys-config" "$STAGE\remote-server\sys-config" @()
}

# 安装脚本 + 服务器说明（LF 化）
Copy-Item "$ASSETS\remote-install.sh" "$STAGE\remote-server\install.sh"     -Force
Copy-Item "$ASSETS\SERVER-NOTES.md"   "$STAGE\remote-server\SERVER-NOTES.md" -Force
$ri = Get-Content "$ASSETS\remote-install.sh" -Raw
[System.IO.File]::WriteAllText("$STAGE\remote-server\install.sh", ($ri -replace "`r`n","`n"))

# ---- [7] 根 README + 删除旧 staging ----
Step '[7/8] 写入根 README 并删除旧灾备包'
Copy-Item "$ASSETS\README.md" "$STAGE\README.md" -Force
if (-not $KeepStage) {
  Remove-Item $OLDH -Recurse -Force
  Write-Host '  旧灾备包 staging（dr-build-old-hold）已删除'
} else {
  Write-Host "  -KeepStage：旧 staging 保留于 $OLDH"
}

# ---- [8] 生成 zip ----
Step '[8/8] 压缩生成灾备包 zip'
if (-not (Test-Path $OUTDIR)) { New-Item $OUTDIR -ItemType Directory -Force | Out-Null }
if (Test-Path $ZIP) { Remove-Item $ZIP -Force }
$oldLimit = $ProgressPreference; $ProgressPreference = 'SilentlyContinue'
Compress-Archive -Path "$STAGE\*" -DestinationPath $ZIP -CompressionLevel Fastest
$ProgressPreference = $oldLimit
$z = Get-Item $ZIP
Write-Host ''
Write-Host ("✅ 灾备包已生成：{0}  ({1:N0} MB)" -f $ZIP, ($z.Length/1MB)) -ForegroundColor Green

# 汇总
$m = Get-ChildItem $STAGE -Recurse -File | Measure-Object Length -Sum
Write-Host ("staging 汇总：{0:N0} MB，{1} 文件" -f ($m.Sum/1MB), $m.Count)
if (-not $KeepStage) { Remove-Item $STAGE -Recurse -Force; Write-Host 'staging 已清理（zip 为最终交付物）' }
