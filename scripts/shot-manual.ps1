$ErrorActionPreference='Continue'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class W2 {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R2 r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f,uint x,uint y,uint d,int e);
  public struct R2 { public int L, T, Rt, B; }
}
'@
$out='B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP\easyrdp-site\assets\img\manual'
New-Item $out -ItemType Directory -Force | Out-Null
$p = Get-Process easyrdp-app | Where-Object {$_.MainWindowHandle -ne 0} | Select-Object -First 1
if(-not $p){ 'NO WINDOW'; exit 1 }
$hwnd=$p.MainWindowHandle
"WIND $hwnd" | Out-File "$out\..\shot.log" -Encoding utf8

function Focus-App { [W2]::SetForegroundWindow($hwnd) | Out-Null; Start-Sleep -Milliseconds 600 }

function Find-Button([string]$name) {
  $root=[Windows.Automation.AutomationElement]::FromHandle($hwnd)
  $c=New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty,$name)
  $el=$root.FindFirst([Windows.Automation.TreeScope]::Descendants,$c)
  if($el){ return $el }
  # 兜底：包含匹配
  $c2=New-Object Windows.Automation.PropertyCondition($true,[Windows.Automation.AutomationElement]::TrueCondition)
  $all=$root.FindAll([Windows.Automation.TreeScope]::Descendants,$c2)
  foreach($e in $all){ if($e.Current.Name -like "*$name*"){ return $e } }
  return $null
}
function Invoke-Button([string]$name) {
  $el=Find-Button $name
  if(-not $el){ return "MISS:$name" }
  try { ($el.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern)).Invoke(); Focus-App; return "OK:$name" }
  catch {
    # 兜底：坐标点击
    $r=$el.Current.BoundingRectangle
    if($r.Width -gt 0){
      $x=[int]($r.X+$r.Width/2); $y=[int]($r.Y+$r.Height/2)
      [W2]::SetCursorPos($x,$y)|Out-Null; Start-Sleep -Milliseconds 120
      [W2]::mouse_event(2,0,0,0,0); [W2]::mouse_event(4,0,0,0,0)
      Focus-App; return "CLICK:$name"
    }
    return "FAIL:$name"
  }
}
function Shot([string]$file) {
  Start-Sleep -Milliseconds 900
  Focus-App
  $r=New-Object W2+R2
  [W2]::GetWindowRect($hwnd,[ref]$r)|Out-Null
  $w=$r.Rt-$r.L; $h=$r.B-$r.T
  $bmp=New-Object System.Drawing.Bitmap($w,$h)
  $g=[System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.L,$r.T,0,0,(New-Object System.Drawing.Size($w,$h)))
  $bmp.Save("$out\$file",[System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
  "SHOT:$file ($w x $h)"
}
function Send-Key([string]$k){ Focus-App; Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.SendKeys]::SendWait($k) }

$log=@()
# 1) 关掉任务栏固定引导（若在）
$log += Invoke-Button '不再提示'
Start-Sleep -Milliseconds 500
# 2) 主界面（新建连接表单）
$log += Shot 'connections.png'
# 3) 设置面板
$log += Invoke-Button '设置'
$log += Shot 'settings.png'
Send-Key '{ESC}'
# 4) 反馈
$log += Invoke-Button '反馈'
$log += Shot 'feedback.png'
Send-Key '{ESC}'
# 5) 导入 .rdp（文件选择器）
$log += Invoke-Button '导入 .rdp'
$log += Shot 'import-rdp.png'
Send-Key '{ESC}'
Start-Sleep -Milliseconds 600
Send-Key '{ESC}'
# 6) 导出
$log += Invoke-Button '导出'
$log += Shot 'export.png'
Send-Key '{ESC}'
Start-Sleep -Milliseconds 600
Send-Key '{ESC}'
# 7) 导入 JSON
$log += Invoke-Button '导入JSON'
$log += Shot 'import-json.png'
Send-Key '{ESC}'
Start-Sleep -Milliseconds 600
Send-Key '{ESC}'
# 8) 模板库
$log += Invoke-Button '模板库'
$log += Shot 'templates.png'
Send-Key '{ESC}'
Start-Sleep -Milliseconds 600
Send-Key '{ESC}'
# 9) 新建分组（分组管理）
$log += Invoke-Button '新建分组'
$log += Shot 'groups.png'
Send-Key '{ESC}'
Start-Sleep -Milliseconds 600
Send-Key '{ESC}'
$log -join ' | ' | Out-File "$out\..\shot.log" -Append -Encoding utf8
'DONE'
