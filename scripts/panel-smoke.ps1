# M0-3 panel smoke test
# Usage: powershell -ExecutionPolicy Bypass -File scripts\panel-smoke.ps1 [-SkipBuild]

param([switch]$SkipBuild)

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root "target\release\quickphrase.exe"
$log = Join-Path $env:LOCALAPPDATA ("QuickPhrase\logs\qp-{0}.log" -f (Get-Date -Format yyyy-MM-dd))

if (-not $SkipBuild) {
    Push-Location $root
    try {
        & cargo build --release
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    } finally {
        Pop-Location
    }
}
if (-not (Test-Path $exe)) { throw "missing $exe" }

Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class QPPanelSmoke {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll", EntryPoint="PostMessageW")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

$results = New-Object System.Collections.Generic.List[object]
function Check([string]$Name, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
}

function Find-Windows {
    $script:main = [IntPtr]::Zero
    $script:panel = [IntPtr]::Zero
    $callback = [QPPanelSmoke+EnumProc]{
        param($hwnd, $lparam)
        $class = New-Object System.Text.StringBuilder 256
        [void][QPPanelSmoke]::GetClassName($hwnd, $class, 256)
        if ($class.ToString() -eq "QuickPhrase.MainWindow") { $script:main = $hwnd }
        if ($class.ToString() -eq "QuickPhrase.Panel") { $script:panel = $hwnd }
        return $true
    }
    [void][QPPanelSmoke]::EnumWindows($callback, [IntPtr]::Zero)
}

function Find-Edit([IntPtr]$Panel) {
    $script:edit = [IntPtr]::Zero
    $callback = [QPPanelSmoke+EnumProc]{
        param($hwnd, $lparam)
        $class = New-Object System.Text.StringBuilder 256
        [void][QPPanelSmoke]::GetClassName($hwnd, $class, 256)
        if ($class.ToString() -eq "Edit") { $script:edit = $hwnd }
        return $true
    }
    [void][QPPanelSmoke]::EnumChildWindows($Panel, $callback, [IntPtr]::Zero)
    return $script:edit
}

# 1. panel bench
$benchLog = Join-Path $env:LOCALAPPDATA ("QuickPhrase\logs\qp-{0}.log" -f (Get-Date -Format yyyy-MM-dd))
$beforeBench = 0
if (Test-Path $benchLog) { $beforeBench = (Get-Content $benchLog).Count }
$bench = Start-Process -FilePath $exe -ArgumentList "--panel-bench" -PassThru -Wait
$benchLines = @()
if (Test-Path $benchLog) { $benchLines = Get-Content $benchLog | Select-Object -Skip $beforeBench }
$benchText = $benchLines -join "`n"
$p95 = $null
if ($benchText -match "p95=(\d+)us") { $p95 = [int]$Matches[1] }
Check "panel bench exits 0" ($bench.ExitCode -eq 0) ("exit=" + $bench.ExitCode)
Check "panel p95 <= 150 ms" ($null -ne $p95 -and $p95 -le 150000) ("p95=" + $p95 + "us")

if ([QPPanelSmoke]::GetForegroundWindow() -eq [IntPtr]::Zero) {
    $results | Format-Table -AutoSize
    "SKIP: no foreground window in this desktop session"
    exit 3
}

# 2. show via hotkey message
$app = Start-Process -FilePath $exe -PassThru
Start-Sleep -Milliseconds 900
$before = 0
if (Test-Path $log) { $before = (Get-Content $log).Count }

Find-Windows
if ($main -eq [IntPtr]::Zero) { throw "main window not found" }
[void][QPPanelSmoke]::PostMessage($main, 0x0312, [IntPtr]1, [IntPtr]::Zero)
Start-Sleep -Milliseconds 900
Find-Windows
Check "panel visible after hotkey" ($panel -ne [IntPtr]::Zero -and [QPPanelSmoke]::IsWindowVisible($panel)) ""

# 3. screenshot evidence
if ($panel -ne [IntPtr]::Zero) {
    $rect = New-Object QPPanelSmoke+RECT
    [void][QPPanelSmoke]::GetWindowRect($panel, [ref]$rect)
    Check "panel is 520x420 at 100% DPI" (($rect.Right - $rect.Left) -eq 520 -and ($rect.Bottom - $rect.Top) -eq 420) ("$($rect.Right - $rect.Left)x$($rect.Bottom - $rect.Top)")
}

# 4. Esc hides
$edit = Find-Edit $panel
[void][QPPanelSmoke]::PostMessage($edit, 0x0100, [IntPtr]0x1B, [IntPtr]::Zero)
Start-Sleep -Milliseconds 600
Find-Windows
Check "Esc hides panel" (-not [QPPanelSmoke]::IsWindowVisible($panel)) ""

# 5. Enter commits first item and hides
[void][QPPanelSmoke]::PostMessage($main, 0x0312, [IntPtr]1, [IntPtr]::Zero)
Start-Sleep -Milliseconds 900
Find-Windows
$edit = Find-Edit $panel
[void][QPPanelSmoke]::PostMessage($edit, 0x0100, [IntPtr]0x0D, [IntPtr]::Zero)
Start-Sleep -Milliseconds 600
Find-Windows
$newLines = @()
if (Test-Path $log) { $newLines = Get-Content $log | Select-Object -Skip $before }
$newText = $newLines -join "`n"
Check "Enter hides panel" (-not [QPPanelSmoke]::IsWindowVisible($panel)) ""
Check "commit logged" ($newText -match "panel commit: title=") ""

# 6. close app
[void][QPPanelSmoke]::PostMessage($main, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds 700
Check "app exits" ($app.HasExited) ("exit=" + $(if ($app.HasExited) { $app.ExitCode } else { "n/a" }))

$failed = @($results | Where-Object { -not $_.Ok })
$results | Format-Table -AutoSize
"--- panel log ---"
$newLines

if ($failed.Count -gt 0) {
    "FAIL: $($failed.Count) check(s)"
    exit 1
}
"PASS: $($results.Count) checks"
exit 0
