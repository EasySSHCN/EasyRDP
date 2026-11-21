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
public class W4 {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R4 r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  public struct R4 { public int L, T, Rt, B; }
  public static List<IntPtr> DlgList = new List<IntPtr>();
  public static uint TargetPid;
  public static bool CB(IntPtr h, IntPtr l) {
    uint pid; GetWindowThreadProcessId(h, out pid);
    if (pid == TargetPid && IsWindowVisible(h)) {
      StringBuilder cn = new StringBuilder(256); GetClassName(h, cn, 256);
      StringBuilder tt = new StringBuilder(256); GetWindowText(h, tt, 256);
      if (cn.ToString() == "#32770") { DlgList.Add(h); }
    }
    return true;
  }
  public static List<IntPtr> Dlgs(uint pid) { DlgList.Clear(); TargetPid = pid; EnumWindows(CB, IntPtr.Zero); return DlgList; }
}
'@
$out='B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP\easyrdp-site\assets\img\manual'
$p = Get-Process easyrdp-app | Where-Object {$_.MainWindowHandle -ne 0} | Select-Object -First 1
$hwnd=$p.MainWindowHandle; $pid2=[uint32]$p.Id
$log=@()

function Close-FileDialogs {
  $ds=[W4]::Dlgs($pid2)
  foreach($d in $ds){ [W4]::PostMessage($d,0x0010,[IntPtr]::Zero,[IntPtr]::Zero) | Out-Null }  # WM_CLOSE
  if($ds.Count -gt 0){ Start-Sleep -Milliseconds 900 }
  return $ds.Count
}
function Focus-App { [W4]::SetForegroundWindow($hwnd)|Out-Null; Start-Sleep -Milliseconds 500 }
function Invoke-Button([string]$name) {
  $root=[Windows.Automation.AutomationElement]::FromHandle($hwnd)
  $all=$root.FindAll([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.Condition]::TrueCondition)
  foreach($e in $all){ $n=$e.Current.Name; if($n -and $n.Contains($name)){
    try{ ($e.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern)).Invoke(); return "OK:$name" }catch{ return "NOPAT:$name" } } }
  return "MISS:$name"
}
function Shot([string]$file) {
  # 等待稳定，判定目标窗口：优先进程内 #32770 对话框，否则主窗口
  Start-Sleep -Milliseconds 1600
  $ds=[W4]::Dlgs($pid2)
  $tHwnd=$hwnd
  if($ds.Count -gt 0){ $tHwnd=$ds[0] }
  [W4]::SetForegroundWindow($tHwnd)|Out-Null; Start-Sleep -Milliseconds 700
  $r=New-Object W4+R4
  [W4]::GetWindowRect($tHwnd,[ref]$r)|Out-Null
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
  $ds=[W4]::Dlgs($pid2)
  if($ds.Count -gt 0){ foreach($d in $ds){ [W4]::PostMessage($d,0x0010,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null }; Start-Sleep -Milliseconds 900 }
  else { Focus-App; [System.Windows.Forms.SendKeys]::SendWait('{ESC}'); Start-Sleep -Milliseconds 700 }
}

$log += "clean=" + (Close-FileDialogs)
# 设置（应用内弹窗）
$log += Invoke-Button '设置';      $log += Shot 'settings.png';    After-Modal
# 导入 .rdp（OS 打开对话框）
$log += Invoke-Button '导入 .rdp'; $log += Shot 'import-rdp.png';  After-Modal
# 导出（OS 另存为）
$log += Invoke-Button '导出';      $log += Shot 'export.png';      After-Modal
# 导入 JSON（OS 打开）
$log += Invoke-Button '导入JSON';  $log += Shot 'import-json.png'; After-Modal
# 模板库（应用内弹窗）
$log += Invoke-Button '模板库';    $log += Shot 'templates.png';   After-Modal
# 新建分组（应用内弹窗）
$log += Invoke-Button '新建分组';  $log += Shot 'groups.png';      After-Modal
$log -join ' | ' | Out-File "$out\..\shot2.log" -Encoding utf8
'DONE'
