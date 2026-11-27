$ErrorActionPreference='Continue'
$log='B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP\scripts\shot-run.log'
"start $(Get-Date -Format T)" | Out-File $log -Encoding utf8
Set-Location 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasyRDP\easyrdp-client\dist\EasyRDP-win64'
$env:WEBVIEW2_USER_DATA_FOLDER="$env:TEMP\erdp-shot"
$p=Start-Process '.\easyrdp-app.exe' -PassThru
Start-Sleep -Seconds 12
if($p.HasExited){ "EXITED $($p.ExitCode)" | Out-File $log -Append -Encoding utf8 } else {
  "RUNNING $($p.Id) hwnd=$($p.MainWindowHandle) title=$($p.MainWindowTitle)" | Out-File $log -Append -Encoding utf8
  Add-Type @'
using System;
using System.Runtime.InteropServices;
public class W {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public struct R { public int L, T, Rt, B; }
}
'@
  [W]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
  Start-Sleep -Milliseconds 800
  $r=New-Object W+R
  [W]::GetWindowRect($p.MainWindowHandle,[ref]$r)|Out-Null
  "RECT $($r.L),$($r.T),$($r.Rt),$($r.B)" | Out-File $log -Append -Encoding utf8
}
"end $(Get-Date -Format T)" | Out-File $log -Append -Encoding utf8
