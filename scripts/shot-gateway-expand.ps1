$ErrorActionPreference='Continue'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class W6 {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R6 r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  public struct R6 { public int L, T, Rt, B; }
}
'@
$out='B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP\easyrdp-site\assets\img\manual'
$p = Get-Process | Where-Object {$_.ProcessName -like 'easyrdp*' -and $_.MainWindowHandle -ne 0} | Select-Object -First 1
$hwnd=$p.MainWindowHandle
[W6]::SetForegroundWindow($hwnd)|Out-Null; Start-Sleep -Milliseconds 600

$root=[Windows.Automation.AutomationElement]::FromHandle($hwnd)
function Find-Name([string]$needle){
  $all=$root.FindAll([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.Condition]::TrueCondition)
  foreach($e in $all){ $n=$e.Current.Name; if($n -and $n.Contains($needle)){ return $e } }
  return $null
}

# 1) 判断展开状态：能找到「网关地址」= 已展开
$gwField = Find-Name '网关地址'
if(-not $gwField){
  $sum = Find-Name '网关（可选）'
  if($sum){
    $r=$sum.Current.BoundingRectangle
    $cx=[int]($r.X+$r.Width/2); $cy=[int]($r.Y+$r.Height/2)
    [W6]::SetCursorPos($cx,$cy)|Out-Null; Start-Sleep -Milliseconds 250
    [W6]::mouse_event(2,0,0,0,[UIntPtr]::Zero); Start-Sleep -Milliseconds 80; [W6]::mouse_event(4,0,0,0,[UIntPtr]::Zero)
    "CLICK-expand $cx,$cy"
    Start-Sleep -Milliseconds 900
    $gwField = Find-Name '网关地址'
  }
}
"expanded=$([bool]$gwField)"

# 2) 滚动让网关区域尽量完整入画：以折叠区位置为轴，先上滚到顶再下滚
if($gwField){
  $r=$gwField.Current.BoundingRectangle
  if(-not $r.IsEmpty){
    $cx=[int]($r.X+$r.Width/2); $cy=[int]($r.Y+$r.Height/2)
    [W6]::SetCursorPos($cx,$cy)|Out-Null; Start-Sleep -Milliseconds 200
    for($i=0;$i -lt 12;$i++){ [W6]::mouse_event(0x0800,0,0,120,[UIntPtr]::Zero); Start-Sleep -Milliseconds 60 }  # 上滚回顶
    Start-Sleep -Milliseconds 400
    for($i=0;$i -lt 8;$i++){ [W6]::mouse_event(0x0800,0,0,[uint32]4294967176,[UIntPtr]::Zero); Start-Sleep -Milliseconds 120 } # 下滚 8 格
    Start-Sleep -Milliseconds 800
  }
}

# 3) 截图 + 裁剪左侧（抹隐私）
$r2raw=New-Object W6+R6
$ok=[W6]::GetWindowRect($hwnd,[ref]$r2raw)
"L=$([int]$r2raw.L) T=$([int]$r2raw.T) Rt=$([int]$r2raw.Rt) B=$([int]$r2raw.B) ok=$ok"
$w=[int]$r2raw.Rt - [int]$r2raw.L; $h=[int]$r2raw.B - [int]$r2raw.T
$bmp=New-Object System.Drawing.Bitmap($w,$h)
$g=[System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen([int]$r2raw.L,[int]$r2raw.T,0,0,(New-Object System.Drawing.Size($w,$h)))
$cw = $bmp.Width - 330
if($cw -lt 50){ $cw = 50 }
$crop=$bmp.Clone((New-Object System.Drawing.Rectangle(330,0,$cw,$bmp.Height)), $bmp.PixelFormat)
$crop.Save("$out\gateway.png",[System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose(); $crop.Dispose()
"CROP-SAVED ${cw}x$($bmp.Height)"
