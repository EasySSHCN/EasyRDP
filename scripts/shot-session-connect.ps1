$ErrorActionPreference='Continue'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class W6 {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f,uint dx,uint dy,uint d,int e);
}
'@
[W6]::SetProcessDPIAware()|Out-Null
$exe='P:\Program Files (x86)\EasyRDP\EasyRDP-0.4.1-portable.exe'
Get-Process easyrdp-app -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
Unblock-File $exe -ErrorAction SilentlyContinue
Start-Process $exe
Start-Sleep -Seconds 12
$p = Get-Process easyrdp-app | Where-Object {$_.MainWindowHandle -ne 0} | Select-Object -First 1
if(-not $p){ 'NO WINDOW'; exit 1 }
$hwnd=$p.MainWindowHandle
"WIND=$hwnd PID=$($p.Id)"
[W6]::SetForegroundWindow($hwnd)|Out-Null; Start-Sleep -Milliseconds 800
# 等 UIA 就绪
for($i=0;$i -lt 60;$i++){
  try{
    $rt=[Windows.Automation.AutomationElement]::FromHandle($hwnd)
    $c=New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty,'⚙ 设置')
    if($rt.FindFirst([Windows.Automation.TreeScope]::Descendants,$c)){ break }
  }catch{}
  Start-Sleep -Milliseconds 500
}
# 找连接条目 192.168.200.26
$root=[Windows.Automation.AutomationElement]::FromHandle($hwnd)
$all=$root.FindAll([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.Condition]::TrueCondition)
$el=$null
foreach($e in $all){ if($e.Current.Name -eq '192.168.200.26'){ $el=$e; break } }
if(-not $el){ 'CONN NOT FOUND'; exit 1 }
$r=$el.Current.BoundingRectangle
$x=[int]($r.X+$r.Width/2); $y=[int]($r.Y+$r.Height/2)
"CLICK $x,$y"
[W6]::SetCursorPos($x,$y)|Out-Null; Start-Sleep -Milliseconds 200
[W6]::mouse_event(2,0,0,0,0);[W6]::mouse_event(4,0,0,0,0);Start-Sleep -Milliseconds 120
[W6]::mouse_event(2,0,0,0,0);[W6]::mouse_event(4,0,0,0,0)
'CLICKED'
