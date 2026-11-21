$ErrorActionPreference='Continue'
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class W9 {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R9 r);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f,uint dx,uint dy,uint d,int e);
  public struct R9 { public int L, T, Rt, B; }
  public static IntPtr Found;
  public static uint TargetPid;
  public static bool CB(IntPtr h, IntPtr l) {
    uint pid; GetWindowThreadProcessId(h, out pid);
    if (pid == TargetPid) { Found = h; return false; }
    return true;
  }
  public static IntPtr AnyOf(uint pid){ Found=IntPtr.Zero; TargetPid=pid; EnumWindows(CB, IntPtr.Zero); return Found; }
}
'@
[W9]::SetProcessDPIAware()|Out-Null
function Grab([int]$L,[int]$T,[int]$Rt,[int]$B,[string]$file){
  $w=$Rt-$L; $h=$B-$T
  $bmp=New-Object System.Drawing.Bitmap($w,$h)
  $g=[System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($L,$T,0,0,(New-Object System.Drawing.Size($w,$h)))
  $bmp.Save($file,[System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
}
$out='B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP\easyrdp-site\assets\img\manual'
# 1) win-key：菜单开着，截会话窗+菜单区（L597 T320 R2180 B580）
Grab 597 320 2180 580 "$out\win-key.png"
# 2) 关菜单（点空白处）→ 截会话全窗 fullscreen
[W9]::SetCursorPos(1200,800)|Out-Null
[W9]::mouse_event(2,0,0,0,0);[W9]::mouse_event(4,0,0,0,0)
Start-Sleep -Milliseconds 800
Grab 597 320 1963 1120 "$out\fullscreen.png"
# 3) disconnect：裁标题栏右侧特写（含发送/─/□/红✕），2x 放大
$src=New-Object System.Drawing.Bitmap("$out\fullscreen.png")
$cap=$src.Clone((New-Object System.Drawing.Rectangle(1100,0,863,60)),($src.PixelFormat))
$big=New-Object System.Drawing.Bitmap($cap,1730,120)
$big.Save("$out\disconnect.png",[System.Drawing.Imaging.ImageFormat]::Png)
$src.Dispose();$cap.Dispose();$big.Dispose()
# 4) 点红 ✕ 断开会话（窗口右上）
[W9]::SetCursorPos(1938,337)|Out-Null; Start-Sleep -Milliseconds 200
[W9]::mouse_event(2,0,0,0,0);[W9]::mouse_event(4,0,0,0,0)
Start-Sleep -Seconds 3
'DONE'
