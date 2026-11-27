# next-version.ps1 —— EasyRDP 版本递进规则（v.1.2.3 三级规则，2026-11-16 用户口径）
# 【版本号三级递进规则（全文，不得违反）】
#   1) 一级版本号（major，最高级）：满足 10 次二级版本号更新才进行升级（minor 满 10 → major+1，minor/patch 归零）
#   2) 二级版本号（minor，中级）：满足 10 次三级版本号更新才进行升级（patch 满 10 → minor+1，patch 归零）
#   3) 三级版本号（patch）：需求池每 3 个可开发的需求则推进一次三级版本升级；
#      或用户在会话中指定专门要求发布版本升级；每升级 10 次就推动升级一次二级版本号
#   4) 里程碑规则（-Minor 开关）：需求池累计需求记录每满 10 条，本次发布升 minor+1、
#      patch 归零（由 release-check.ps1 依据池条目计数自动传入，如 0.1.4 → 0.2.0）
# 用法: powershell -File scripts\next-version.ps1 -Current 0.1.0          → 0.1.1
#       powershell -File scripts\next-version.ps1 -Current 0.1.4 -Minor  → 0.2.0
param(
    [Parameter(Mandatory=$true)][string]$Current,
    [switch]$Minor
)
$v = [version]$Current
$maj = $v.Major
$min = $v.Minor
$pat = if ($v.Build -lt 0) { 0 } else { $v.Build }
# 里程碑（池累计需求满 10 条）：直接升 minor+1、patch 归零
if ($Minor) { "$maj.$($min + 1).0"; exit }
# minor 已累积到 10 → 本次发布直接升分支版本（major+1,minor/patch 归零）
if ($min -ge 10) { "$($maj + 1).0.0"; exit }
$pat++
if ($pat -gt 10) { $pat = 0; $min++ }
"$maj.$min.$pat"
