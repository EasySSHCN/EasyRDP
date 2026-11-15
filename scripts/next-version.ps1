# next-version.ps1 —— EasyRDP 版本递进规则
# 规则(沿用 EasySSH v5 口径):
#   1) 常规发布:patch + 1(如 0.1.0 → 0.1.1)
#   2) patch 累积到 10(如 v0.1.10)后,下一次发布升 minor+1、patch 归零(0.1.10 → 0.2.0)
#   3) minor 累积到 10(如 v0.10.0)后,下一次发布升 major+1、patch 归零(0.10.0 → 1.0.0)
# 用法: powershell -File scripts\next-version.ps1 -Current 0.1.0   →  输出 0.1.1
param([Parameter(Mandatory=$true)][string]$Current)
$v = [version]$Current
$maj = $v.Major
$min = $v.Minor
$pat = if ($v.Build -lt 0) { 0 } else { $v.Build }
# minor 已累积到 10 → 本次发布直接升分支版本(major+1,minor/patch 归零)
if ($min -ge 10) { "$($maj + 1).0.0"; exit }
$pat++
if ($pat -gt 10) { $pat = 0; $min++ }
"$maj.$min.$pat"
