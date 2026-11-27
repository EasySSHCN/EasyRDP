$ErrorActionPreference='Continue'
$dir='B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP\easyrdp-site\manual'
# 1) 所有手册页：toc 加「导出 JSON」+ 徽章 v0.3.13→v0.4.1
Get-ChildItem $dir -Filter *.html | ForEach-Object {
  $p=$_.FullName
  $t=[IO.File]::ReadAllText($p,[Text.UTF8Encoding]::new($false))
  $ch=$false
  if($t.Contains('<a href="export.html">导出连接</a>') -and -not $t.Contains('export-json.html')){
    $t=$t.Replace('<a href="export.html">导出连接</a>','<a href="export.html">导出连接</a>'+"`n"+'      <a href="export-json.html">导出 JSON</a>'); $ch=$true
  }
  if($t.Contains('>v0.3.13</span>')){ $t=$t.Replace('>v0.3.13</span>','>v0.4.1</span>'); $ch=$true }
  if($ch){ [IO.File]::WriteAllText($p,$t,[Text.UTF8Encoding]::new($false)); "toc+badge: $($_.Name)" }
}
# 2) index.html 表格加行
$ix="$dir\index.html"
$t=[IO.File]::ReadAllText($ix,[Text.UTF8Encoding]::new($false))
$row='        <tr><td style="padding:6px">导出 JSON</td><td style="padding:6px"><a href="export-json.html">export-json.html</a></td><td style="padding:6px">导出</td></tr>'
if(-not $t.Contains('export-json.html')){
  $anchor='        <tr><td style="padding:6px">导入 JSON</td><td style="padding:6px"><a href="import-json.html">import-json.html</a></td><td style="padding:6px">导入JSON</td></tr>'
  $t=$t.Replace($anchor,$anchor+"`n"+$row); [IO.File]::WriteAllText($ix,$t,[Text.UTF8Encoding]::new($false)); 'index row added'
}
'---'
Select-String -Path "$dir\*.html" -Pattern 'v0\.3\.13' | Measure-Object | % Count
