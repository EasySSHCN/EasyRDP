# 网关手册页截图：打开「＋ 新建连接」弹窗 → 点开「网关（可选）」折叠区 → 截图 gateway.png
# 前置：easyrdp-app 已启动（本机 WebView2 正常时）。用法：powershell -ExecutionPolicy Bypass -File shot-manual-gateway.ps1
$ErrorActionPreference='Continue'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @'
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class W5 {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R5 r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  public struct R5 { public int L, T, Rt, B; }
  public static List<IntPtr> DlgList = new List<IntPtr>();
  public static uint TargetPid;
  public static bool CB(IntPtr h, IntPtr l) {
    uint pid; GetWindowThreadProcessId(h, out pid);
    if (pid == TargetPid && IsWindowVisible(h)) {
      StringBuilder cn = new StringBuilder(256); GetClassName(h, cn, 256);
      if (cn.ToString() == "#32770") { DlgList.Add(h); }
    }
    return true;
  }
  public static List<IntPtr> Dlgs(uint pid) { DlgList.Clear(); TargetPid = pid; EnumWindows(CB, IntPtr.Zero); return DlgList; }
}
'@
$out='B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP\easyrdp-site\assets\img\manual'
$p = Get-Process | Where-Object {$_.ProcessName -like 'easyrdp*' -and $_.MainWindowHandle -ne 0} | Select-Object -First 1
if(-not $p){ throw 'EasyRDP 进程未找到（窗口未打开？）' }
$hwnd=$p.MainWindowHandle; $pid2=[uint32]$p.Id
$log=@()

function Focus-App { [W5]::SetForegroundWindow($hwnd)|Out-Null; Start-Sleep -Milliseconds 500 }
function Invoke-Button([string]$name) {
  $root=[Windows.Automation.AutomationElement]::FromHandle($hwnd)
  $all=$root.FindAll([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.Condition]::TrueCondition)
  foreach($e in $all){ $n=$e.Current.Name; if($n -and $n.Contains($name)){
    try{ ($e.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern)).Invoke(); return "OK:$name" }catch{ return "NOPAT:$name" } } }
  return "MISS:$name"
}
function Click-Element([string]$name) {
  $root=[Windows.Automation.AutomationElement]::FromHandle($hwnd)
  $all=$root.FindAll([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.Condition]::TrueCondition)
  foreach($e in $all){ $n=$e.Current.Name; if($n -and $n.Contains($name)){
    try{ $pt=$e.GetClickablePoint(); [W5]::SetCursorPos([int]$pt.X,[int]$pt.Y)|Out-Null; Start-Sleep -Milliseconds 200
         [W5]::mouse_event(2,0,0,0,[UIntPtr]::Zero); [W5]::mouse_event(4,0,0,0,[UIntPtr]::Zero); return "CLICK:$name" }catch{ return "NOPT:$name" } } }
  return "MISS:$name"
}
function Shot([string]$file) {
  Start-Sleep -Milliseconds 1600
  $ds=[W5]::Dlgs($pid2)
  $tHwnd=$hwnd
  if($ds.Count -gt 0){ $tHwnd=$ds[0] }
  [W5]::SetForegroundWindow($tHwnd)|Out-Null; Start-Sleep -Milliseconds 700
  $r=New-Object W5+R5
  [W5]::GetWindowRect($tHwnd,[ref]$r)|Out-Null
  $w=$r.Rt-$r.L; $h=$r.B-$r.T
  $bmp=New-Object System.Drawing.Bitmap($w,$h)
  $g=[System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.L,$r.T,0,0,(New-Object System.Drawing.Size($w,$h)))
  $bmp.Save("$out\$file",[System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
  $kind = if($tHwnd -eq $hwnd){'app'}else{'dlg'}
  return "SHOT:$file($kind ${w}x${h})"
}
function After-Modal {
  Start-Sleep -Milliseconds 400
  $ds=[W5]::Dlgs($pid2)
  if($ds.Count -gt 0){ foreach($d in $ds){ [W5]::PostMessage($d,0x0010,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null }; Start-Sleep -Milliseconds 900 }
  else { Focus-App; [System.Windows.Forms.SendKeys]::SendWait('{ESC}'); Start-Sleep -Milliseconds 700 }
}

Focus-App
$log += Invoke-Button '新建连接'
Start-Sleep -Milliseconds 1200
# 展开网关折叠区（summary 元素经 UIA 点击）
$log += Click-Element '网关（可选）'
Start-Sleep -Milliseconds 600
# 滚动到网关区域可见（弹窗内 PageDown 少许）
$log += Shot 'gateway.png'
After-Modal
$log -join ' | ' | Out-File "$out\..\shot-gateway.log" -Encoding utf8
'DONE'
