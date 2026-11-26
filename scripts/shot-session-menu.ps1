$ErrorActionPreference='Continue'
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class W8 {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R8 r);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f,uint dx,uint dy,uint d,int e);
  public struct R8 { public int L, T, Rt, B; }
  public static IntPtr Found;
  public static uint TargetPid;
  public static bool CB(IntPtr h, IntPtr l) {
    uint pid; GetWindowThreadProcessId(h, out pid);
    if (pid == TargetPid && IsWindowVisibleCheck(h)) {
      StringBuilder tt = new StringBuilder(256); GetWindowText(h, tt, 256);
      if (tt.ToString().Contains("远程桌面")) { Found = h; return false; }
    }
    return true;
  }
  [DllImport("user32.dll")] private static extern bool IsWindowVisible(IntPtr h);
  private static bool IsWindowVisibleCheck(IntPtr h){ return IsWindowVisible(h); }
  public static IntPtr FindSession(uint pid){ Found=IntPtr.Zero; TargetPid=pid; EnumWindows(CB, IntPtr.Zero); return Found; }
}
'@
[W8]::SetProcessDPIAware()|Out-Null
$p = Get-Process | Where-Object {$_.Name -match '^EasyRDP'} | Select-Object -First 1
$pid2=[uint32]$p.Id
$s=[W8]::FindSession($pid2)
if($s -eq [IntPtr]::Zero){ 'SESSION NOT FOUND'; exit 1 }
$r=New-Object W8+R8
[W8]::GetWindowRect($s,[ref]$r)|Out-Null
"SESSION RECT $($r.L),$($r.T),$($r.Rt),$($r.B)"
# 主窗口句柄与尺寸
$m=$p.MainWindowHandle
$mr=New-Object W8+R8
[W8]::GetWindowRect($m,[ref]$mr)|Out-Null
"MAIN RECT $($mr.L),$($mr.T),$($mr.Rt),$($mr.B)"
# 会话窗口相对主窗口定位「发送」按钮（截图观察：会话窗标题栏右侧，发送≈窗宽-160px, y≈17）
$wx = $r.Rt - 160
$wy = $r.T + 17
"CLICK-SEND $wx,$wy"
[W8]::SetForegroundWindow($s)|Out-Null; Start-Sleep -Milliseconds 600
[W8]::SetCursorPos($wx,$wy)|Out-Null; Start-Sleep -Milliseconds 200
[W8]::mouse_event(2,0,0,0,0);[W8]::mouse_event(4,0,0,0,0)
Start-Sleep -Milliseconds 900
# 截整屏看菜单
$bmp=New-Object System.Drawing.Bitmap(2560,1440)
$g=[System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen(0,0,0,0,(New-Object System.Drawing.Size(2560,1440)))
$bmp.Save('B:\erdp-menu.png',[System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
'MENU SHOT SAVED'
