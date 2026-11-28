$ErrorActionPreference='Continue'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$p = Get-Process easyrdp-app | Where-Object {$_.MainWindowHandle -ne 0} | Select-Object -First 1
if(-not $p){ 'NO PROC'; exit }
"HWND=$($p.MainWindowHandle) PID=$($p.Id) TITLE=$($p.MainWindowTitle)"
$root=[Windows.Automation.AutomationElement]::FromHandle($p.MainWindowHandle)
$all=$root.FindAll([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.Condition]::TrueCondition)
"N=$($all.Count)"
$names=@()
foreach($e in $all){ $n=$e.Current.Name; if($n){ $names+=$n } }
$names | Select-Object -Unique | ForEach-Object { "N:$_" }
