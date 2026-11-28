param([Parameter(Mandatory=$true)][string]$Action)
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
  public struct R5 { public int L, T, Rt, B; }
  public static List<IntPtr> DlgList = new List<IntPtr>();
  public static uint TargetPid;
  public static bool CB(IntPtr h, IntPtr l) {
    uint pid; GetWindowThreadProcessId(h, out pid);
    if (pid == TargetPid && IsWindowVisible(h)) {
      StringBuilder cn = new StringBuilder(256); GetClassName(h, cn, 256);
      if (cn.ToString() == "#32770") DlgList.Add(h);
    }
    return true;
  }
  public static List<IntPtr> Dlgs(uint pid) { DlgList.Clear(); TargetPid = pid; EnumWindows(CB, IntPtr.Zero); return DlgList; }
}
'@
$out='B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP\easyrdp-site\assets\img\manual'
$bin='B:\erdp-shot-bin'
# 1) 干净重启
Get-Process easyrdp-app -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
Start-Process "$bin\easyrdp-app.exe" -WorkingDirectory $bin
Start-Sleep -Seconds 10
$p = Get-Process easyrdp-app | Where-Object {$_.MainWindowHandle -ne 0} | Select-Object -First 1
if(-not $p){ 'NO WINDOW'; exit 1 }
$hwnd=$p.MainWindowHandle; $pid2=[uint32]$p.Id
[W5]::SetForegroundWindow($hwnd)|Out-Null; Start-Sleep -Milliseconds 800
# 等 WebView2 内容可访问：UIA 树出现「⚙ 设置」按钮为止（最多 30s）
$ready=$false
for($i=0;$i -lt 60;$i++){
  try{
    $rt=[Windows.Automation.AutomationElement]::FromHandle($hwnd)
    $c=New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty,'⚙ 设置')
    if($rt.FindFirst([Windows.Automation.TreeScope]::Descendants,$c)){ $ready=$true; break }
  }catch{}
  Start-Sleep -Milliseconds 500
}
"READY=$ready"

function Invoke-Button([string]$name) {
  $root=[Windows.Automation.AutomationElement]::FromHandle($hwnd)
  $all=$root.FindAll([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.Condition]::TrueCondition)
  foreach($e in $all){ $n=$e.Current.Name; if($n -and $n.Contains($name)){
    Write-Host "INVOKE-HIT:[$n]"
    try{ ($e.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern)).Invoke(); return $true }catch{ Write-Host "INVOKE-ERR:$_"; return $false } } }
  Write-Host "INVOKE-MISS:[$name]"
  return $false
}
function Wait-Dlg([int]$ms) {
  $t=0
  while($t -lt $ms){ $ds=[W5]::Dlgs($pid2); if($ds.Count -gt 0){ return $ds[0] }; Start-Sleep -Milliseconds 300; $t+=300 }
  return [IntPtr]::Zero
}
function Shot-Hwnd([IntPtr]$tHwnd,[string]$file) {
  [W5]::SetForegroundWindow($tHwnd)|Out-Null; Start-Sleep -Milliseconds 800
  $r=New-Object W5+R5
  [W5]::GetWindowRect($tHwnd,[ref]$r)|Out-Null
  $w=$r.Rt-$r.L; $h=$r.B-$r.T
  $bmp=New-Object System.Drawing.Bitmap($w,$h)
  $g=[System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.L,$r.T,0,0,(New-Object System.Drawing.Size($w,$h)))
  $bmp.Save("$out\$file",[System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
}
switch($Action){
  'settings'   { [void](Invoke-Button '设置');     Start-Sleep -Milliseconds 1500; Shot-Hwnd $hwnd 'settings.png';    [W5]::SetForegroundWindow($hwnd)|Out-Null; [System.Windows.Forms.SendKeys]::SendWait('{ESC}') }
  'feedback'   { [void](Invoke-Button '反馈');     Start-Sleep -Milliseconds 1500; Shot-Hwnd $hwnd 'feedback.png';    [W5]::SetForegroundWindow($hwnd)|Out-Null; [System.Windows.Forms.SendKeys]::SendWait('{ESC}') }
  'templates'  { [void](Invoke-Button '模板库');   Start-Sleep -Milliseconds 1500; Shot-Hwnd $hwnd 'templates.png';   [W5]::SetForegroundWindow($hwnd)|Out-Null; [System.Windows.Forms.SendKeys]::SendWait('{ESC}') }
  'connections'{ Shot-Hwnd $hwnd 'connections.png' }
  'groups' {
    [void](Invoke-Button '新建分组'); Start-Sleep -Milliseconds 1500
    Shot-Hwnd $hwnd 'groups.png'
    # 点取消关闭 prompt
    $root=[Windows.Automation.AutomationElement]::FromHandle($hwnd)
    $all=$root.FindAll([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.Condition]::TrueCondition)
    foreach($e in $all){ if($e.Current.Name -eq '取消'){ try{ ($e.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern)).Invoke(); break }catch{} } }
  }
  'importrdp' {
    [void](Invoke-Button '导入 .rdp')
    $d=Wait-Dlg 6000
    if($d -ne [IntPtr]::Zero){ Shot-Hwnd $d 'import-rdp.png'; [W5]::PostMessage($d,0x0010,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null; Start-Sleep -Milliseconds 900; 'DLG-SHOT' }
    else { Shot-Hwnd $hwnd 'import-rdp.png'; 'NO-DLG-APP-SHOT' }
  }
  'export' {
    [void](Invoke-Button '导出')
    $d=Wait-Dlg 8000
    if($d -ne [IntPtr]::Zero){ Start-Sleep -Milliseconds 2500; Shot-Hwnd $d 'export.png'; [W5]::PostMessage($d,0x0010,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null; Start-Sleep -Milliseconds 900; 'DLG-SHOT' }
    else { Shot-Hwnd $hwnd 'export.png'; 'NO-DLG-APP-SHOT' }
  }
  'importjson' {
    [void](Invoke-Button '导入JSON')
    $d=Wait-Dlg 6000
    if($d -ne [IntPtr]::Zero){ Shot-Hwnd $d 'import-json.png'; [W5]::PostMessage($d,0x0010,[IntPtr]::Zero,[IntPtr]::Zero)|Out-Null; Start-Sleep -Milliseconds 900; 'DLG-SHOT' }
    else { Shot-Hwnd $hwnd 'import-json.png'; 'NO-DLG-APP-SHOT' }
  }
}
'OK'
