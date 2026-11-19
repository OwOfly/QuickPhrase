# UI screenshot helper for local visual QA.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\ui-shot.ps1 -Theme light|dark
param([ValidateSet("light", "dark")] [string]$Theme = "light")

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root "target\release\quickphrase.exe"
$outDir = Join-Path $root "dist\ui-shots"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class QPShotDpi {
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr ctx);
}
"@
# Per-monitor-v2 awareness so rects and PrintWindow use physical pixels.
[void][QPShotDpi]::SetProcessDpiAwarenessContext([IntPtr](-4))
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class QPShot {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int ht, bool repaint);
  [DllImport("user32.dll", EntryPoint="PostMessageW")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

$settings = Join-Path $env:LOCALAPPDATA "QuickPhrase\settings.json"
$original = [IO.File]::ReadAllText($settings)
try {
    $updated = $original -replace '"theme"\s*:\s*"[^"]*"', ('"theme":  "' + $Theme + '"')
    [IO.File]::WriteAllText($settings, $updated, (New-Object Text.UTF8Encoding($false)))

    function Find-Window([string]$Class) {
        $script:found = [IntPtr]::Zero
        $callback = [QPShot+EnumProc]{
            param($hwnd, $lparam)
            $name = New-Object System.Text.StringBuilder 256
            [void][QPShot]::GetClassName($hwnd, $name, 256)
            if ($name.ToString() -eq $Class) { $script:found = $hwnd }
            return $true
        }
        [void][QPShot]::EnumWindows($callback, [IntPtr]::Zero)
        return $script:found
    }

    function Capture([IntPtr]$Hwnd, [string]$Path) {
        [void][QPShot]::SetForegroundWindow($Hwnd)
        Start-Sleep -Milliseconds 350
        $rect = New-Object QPShot+RECT
        [void][QPShot]::GetWindowRect($Hwnd, [ref]$rect)
        $width = $rect.Right - $rect.Left
        $height = $rect.Bottom - $rect.Top
        Write-Output ("rect {0},{1} {2}x{3}" -f $rect.Left, $rect.Top, $width, $height)
        $bitmap = New-Object System.Drawing.Bitmap($width, $height)
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        $hdc = $graphics.GetHdc()
        $ok = [QPShot]::PrintWindow($Hwnd, $hdc, 2)
        $graphics.ReleaseHdc($hdc)
        if (-not $ok) {
            $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, (New-Object System.Drawing.Size($width, $height)))
        }
        $graphics.Dispose()
        $bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
        $bitmap.Dispose()
        Write-Output ("saved {0} {1}x{2}" -f (Split-Path $Path -Leaf), $width, $height)
    }

    Get-Process quickphrase -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 400

    # Panel
    $env:QUICKPHRASE_TEST_ALLOW_UNFOCUSED_PANEL = "1"
    $env:QUICKPHRASE_TEST_NO_INJECT = "1"
    $env:QUICKPHRASE_TEST_NO_ANIMATION = "1"
    $app = Start-Process -FilePath $exe -PassThru
    Start-Sleep -Milliseconds 900
    $main = Find-Window "QuickPhrase.MainWindow"
    if ($main -ne [IntPtr]::Zero) {
        # Panel opens on the cursor's monitor; keep shots on the primary.
        [void][QPShot]::SetCursorPos(1376, 520)
        Start-Sleep -Milliseconds 250
        [void][QPShot]::PostMessage($main, 0x0312, [IntPtr]1, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 900
        $panel = Find-Window "QuickPhrase.Panel"
        if ($panel -ne [IntPtr]::Zero -and [QPShot]::IsWindowVisible($panel)) {
            Capture $panel (Join-Path $outDir ("panel-{0}.png" -f $Theme))
            [void][QPShot]::PostMessage($panel, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
        } else {
            Write-Output "panel not found"
        }
    } else {
        Write-Output "main window not found"
    }
    Start-Sleep -Milliseconds 300
    Get-Process quickphrase -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500

    # Management window: default and minimum size.
    $manage = Start-Process -FilePath $exe -ArgumentList "--manage-test" -PassThru
    Start-Sleep -Milliseconds 900
    $hwnd = Find-Window "QuickPhrase.ManageWindow"
    if ($hwnd -ne [IntPtr]::Zero) {
        $dpi = [QPShot]::GetDpiForWindow($hwnd)
        if ($dpi -eq 0) { $dpi = 96 }
        $scale = $dpi / 96.0
        Capture $hwnd (Join-Path $outDir ("manage-{0}.png" -f $Theme))
        [void][QPShot]::MoveWindow($hwnd, [int](60 * $scale), [int](60 * $scale), [int](800 * $scale), [int](540 * $scale), $true)
        Start-Sleep -Milliseconds 400
        Capture $hwnd (Join-Path $outDir ("manage-{0}-min.png" -f $Theme))
    } else {
        Write-Output "manage window not found"
    }
    Get-Process quickphrase -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500

    # Settings window: default and minimum size.
    $settingsProc = Start-Process -FilePath $exe -ArgumentList "--settings-test" -PassThru
    Start-Sleep -Milliseconds 900
    $hwnd = Find-Window "QuickPhrase.SettingsWindow"
    if ($hwnd -ne [IntPtr]::Zero) {
        $dpi = [QPShot]::GetDpiForWindow($hwnd)
        if ($dpi -eq 0) { $dpi = 96 }
        $scale = $dpi / 96.0
        Capture $hwnd (Join-Path $outDir ("settings-{0}.png" -f $Theme))
        [void][QPShot]::MoveWindow($hwnd, [int](100 * $scale), [int](100 * $scale), [int](500 * $scale), [int](420 * $scale), $true)
        Start-Sleep -Milliseconds 400
        Capture $hwnd (Join-Path $outDir ("settings-{0}-min.png" -f $Theme))
    } else {
        Write-Output "settings window not found"
    }
    Get-Process quickphrase -ErrorAction SilentlyContinue | Stop-Process -Force
} finally {
    [IO.File]::WriteAllText($settings, $original, (New-Object Text.UTF8Encoding($false)))
    Get-Process quickphrase -ErrorAction SilentlyContinue | Stop-Process -Force
}