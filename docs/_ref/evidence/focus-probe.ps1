Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class QPW {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct GUITHREADINFO {
    public int cbSize; public int flags; public IntPtr hwndActive; public IntPtr hwndFocus;
    public IntPtr hwndCapture; public IntPtr hwndMenuOwner; public IntPtr hwndMoveSize;
    public IntPtr hwndCaret; public RECT rcCaret;
  }
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint tid, ref GUITHREADINFO info);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, System.Text.StringBuilder sb, int max);
  [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint flags);
}
"@

function Describe([IntPtr]$h) {
  if ($h -eq [IntPtr]::Zero) { return "NULL" }
  $sb = New-Object System.Text.StringBuilder 256
  [void][QPW]::GetClassName($h, $sb, 256)
  return ("0x{0:X} [{1}]" -f $h.ToInt64(), $sb.ToString())
}

$fg = [QPW]::GetForegroundWindow()
$procId = 0
$tid = [QPW]::GetWindowThreadProcessId($fg, [ref]$procId)
$gti = New-Object QPW+GUITHREADINFO
$gti.cbSize = [Runtime.InteropServices.Marshal]::SizeOf($gti)
$ok = [QPW]::GetGUIThreadInfo($tid, [ref]$gti)
$pname = try { (Get-Process -Id $procId -ErrorAction Stop).ProcessName } catch { "?" }
[pscustomobject]@{
  Process    = "$pname ($procId)"
  Foreground = (Describe $fg)
  GTIOk      = $ok
  Active     = (Describe $gti.hwndActive)
  Focus      = (Describe $gti.hwndFocus)
  Caret      = (Describe $gti.hwndCaret)
  FocusIsOwnChildOfForeground = ([QPW]::GetAncestor($gti.hwndFocus, 2) -eq $fg)
} | Format-List
