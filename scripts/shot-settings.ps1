$ErrorActionPreference='Continue'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class W3 {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R3 r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public struct R3 { public int L, T, Rt, B; }
}
'@
$out='B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP\easyrdp-site\assets\img\manual'
$p = Get-Process easyrdp-app | Where-Object {$_.MainWindowHandle -ne 0} | Select-Object -First 1
$hwnd=$p.MainWindowHandle
$root=[Windows.Automation.AutomationElement]::FromHandle($hwnd)
$all=$root.FindAll([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.Condition]::TrueCondition)
$target=$null
foreach($e in $all){ $n=$e.Current.Name; if($n -and $n.Contains('设置')){ Write-Host "found: [$n]"; $target=$e; break } }
if(-not $target){ 'NOT FOUND'; exit 1 }
($target.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern)).Invoke()
Start-Sleep -Milliseconds 1200
[W3]::SetForegroundWindow($hwnd)|Out-Null; Start-Sleep -Milliseconds 500
$r=New-Object W3+R3
[W3]::GetWindowRect($hwnd,[ref]$r)|Out-Null
$w=$r.Rt-$r.L; $h=$r.B-$r.T
$bmp=New-Object System.Drawing.Bitmap($w,$h)
$g=[System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.L,$r.T,0,0,(New-Object System.Drawing.Size($w,$h)))
$bmp.Save("$out\settings.png",[System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
[System.Windows.Forms.SendKeys]::SendWait('{ESC}')
'SETTINGS SHOT OK'
