$ErrorActionPreference='Stop'
$dir="B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP\easyrdp-site\manual"

# 1) index.html 总目录表插行
$row = '        <tr><td style="padding:6px">新建连接 / 连接中心</td><td style="padding:6px"><a href="connections.html">connections.html</a></td><td style="padding:6px">＋ 新建连接</td></tr>'
$gwrow = '        <tr><td style="padding:6px">新建连接 / 连接中心</td><td style="padding:6px"><a href="connections.html">connections.html</a></td><td style="padding:6px">＋ 新建连接</td></tr>' + "`n" + '        <tr><td style="padding:6px">网关接入（云网关中转）</td><td style="padding:6px"><a href="gateway.html">gateway.html</a></td><td style="padding:6px">新建连接 → 网关（可选）</td></tr>'
$idx = Get-Content "$dir\index.html" -Raw -Encoding UTF8
if($idx.Contains($row) -and -not $idx.Contains('gateway.html')){
  $idx=$idx.Replace($row,$gwrow)
  [IO.File]::WriteAllText("$dir\index.html",$idx,(New-Object Text.UTF8Encoding $false))
  "index OK"
} else { "index SKIP" }

# 2) 各手册页侧栏插链接
Get-ChildItem "$dir\*.html" | Where-Object Name -ne 'index.html' | ForEach-Object {
  $c = [IO.File]::ReadAllText($_.FullName)
  $anchor = '<a href="connections.html">新建连接</a>'
  $ins = $anchor + "`n" + '      <a href="gateway.html">网关接入</a>'
  if($c.Contains($anchor) -and -not $c.Contains('gateway.html')){
    $c = $c.Replace($anchor,$ins)
    [IO.File]::WriteAllText($_.FullName,$c,(New-Object Text.UTF8Encoding $false))
    "side OK: $($_.Name)"
  } else { "side SKIP: $($_.Name)" }
}
