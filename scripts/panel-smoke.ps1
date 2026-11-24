# M0-3 panel smoke test
# Usage: powershell -ExecutionPolicy Bypass -File scripts\panel-smoke.ps1 [-SkipBuild]

param([switch]$SkipBuild)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "smoke-common.ps1")
Assert-QPTestIdle
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root "target\release\quickphrase.exe"

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

$qpRun = New-QPTestRun $exe
$log = $qpRun.Log
try {
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class QPPanelSmoke {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageTimeout(IntPtr h, uint m, IntPtr w, string l, uint flags, uint timeout, out IntPtr result);
  [DllImport("user32.dll", EntryPoint="SendMessageTimeoutW")] public static extern IntPtr SendMessageTimeoutInt(IntPtr h, uint m, IntPtr w, IntPtr l, uint flags, uint timeout, out IntPtr result);
  [DllImport("user32.dll")] public static extern IntPtr MonitorFromWindow(IntPtr h, uint flags);
  [DllImport("user32.dll", EntryPoint="GetMonitorInfoW", CharSet=CharSet.Unicode)] public static extern bool GetMonitorInfo(IntPtr h, ref MONITORINFO m);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr context);
  [DllImport("user32.dll", EntryPoint="PostMessageW")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  public struct RECT { public int Left, Top, Right, Bottom; }
  public struct MONITORINFO { public uint cbSize; public RECT rcMonitor; public RECT rcWork; public uint dwFlags; }
}
"@

$results = New-Object System.Collections.Generic.List[object]
# Measure physical pixels, not DPI-virtualized coordinates.
[void][QPPanelSmoke]::SetProcessDpiAwarenessContext([IntPtr](-4))
function Check([string]$Name, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
}

function Find-Windows {
    $script:main = [IntPtr]::Zero
    $script:panel = [IntPtr]::Zero
    $callback = [QPPanelSmoke+EnumProc]{
        param($hwnd, $unused)
        if (-not (Test-QPTestWindow $hwnd)) { return $true }
        $class = New-Object System.Text.StringBuilder 256
        [void][QPPanelSmoke]::GetClassName($hwnd, $class, 256)
        if ($class.ToString() -eq "QuickPhrase.MainWindow") { $script:main = $hwnd }
        if ($class.ToString() -eq "QuickPhrase.Panel") { $script:panel = $hwnd }
        return $true
    }
    [void][QPPanelSmoke]::EnumWindows($callback, [IntPtr]::Zero)
}

function Open-Panel([int]$Attempts = 3) {
    for ($i = 0; $i -lt $Attempts; $i++) {
        $script:beforeForeground = [QPPanelSmoke]::GetForegroundWindow()
        [void][QPPanelSmoke]::PostMessage($script:main, 0x0312, [IntPtr]1, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 900
        Find-Windows
        if ($script:panel -ne [IntPtr]::Zero -and [QPPanelSmoke]::IsWindowVisible($script:panel)) {
            return $true
        }
        Start-Sleep -Milliseconds 500
    }
    return $false
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
    if ($script:edit -ne [IntPtr]::Zero -and [QPPanelSmoke]::GetParent($script:edit) -ne $Panel) {
        throw "edit parent mismatch"
    }
    return $script:edit
}

function Find-List([IntPtr]$Panel) {
    $script:list = [IntPtr]::Zero
    $callback = [QPPanelSmoke+EnumProc]{
        param($hwnd, $lparam)
        $class = New-Object System.Text.StringBuilder 256
        [void][QPPanelSmoke]::GetClassName($hwnd, $class, 256)
        if ($class.ToString() -eq "ListBox") { $script:list = $hwnd }
        return $true
    }
    [void][QPPanelSmoke]::EnumChildWindows($Panel, $callback, [IntPtr]::Zero)
    return $script:list
}

function Get-ListCount([IntPtr]$List) {
    if ($List -eq [IntPtr]::Zero) { return -1 }
    $result = [IntPtr]::Zero
    $ok = [QPPanelSmoke]::SendMessageTimeoutInt($List, 0x018B, [IntPtr]::Zero, [IntPtr]::Zero, 0x0002, 2000, [ref]$result)
    if ($ok -eq [IntPtr]::Zero) { return -1 }
    return [int]$result
}

# 1. panel bench
$benchLog = $log
$beforeBench = 0
if (Test-Path $benchLog) { $beforeBench = (Get-Content $benchLog -Encoding UTF8).Count }
$bench = Start-QPTest -Arguments "--panel-bench" -Wait
$benchLines = @()
if (Test-Path $benchLog) { $benchLines = Get-Content $benchLog -Encoding UTF8 | Select-Object -Skip $beforeBench }
$benchText = $benchLines -join "`n"
$p95 = $null
if ($benchText -match "p95=(\d+)us") { $p95 = [int]$Matches[1] }
Check "panel bench exits 0" ($bench.ExitCode -eq 0) ("exit=" + $bench.ExitCode)
Check "panel p95 <= 150 ms" ($null -ne $p95 -and $p95 -le 150000) ("p95=" + $p95 + "us")

if ([QPPanelSmoke]::GetForegroundWindow() -eq [IntPtr]::Zero) {
    $results | Format-Table -AutoSize
    $earlyFailed = @($results | Where-Object { -not $_.Ok })
    if ($earlyFailed.Count -gt 0) {
        "FAIL: $($earlyFailed.Count) check(s) before the environment skip"
        exit 1
    }
    "SKIP: no foreground window in this desktop session"
    exit 3
}

# 2. show via hotkey message
$app = Start-QPTest -PanelWithoutInjection
Start-Sleep -Milliseconds 900
$beforeForeground = [QPPanelSmoke]::GetForegroundWindow()
$before = 0
if (Test-Path $log) { $before = (Get-Content $log -Encoding UTF8).Count }

Find-Windows
if ($main -eq [IntPtr]::Zero) { throw "main window not found" }
$opened = Open-Panel 3
$restoreTarget = $beforeForeground
Check "panel visible after hotkey" $opened ""

# 3. size check (scaled by panel DPI, clamped to the work area like place())
if ($panel -ne [IntPtr]::Zero) {
    $rect = New-Object QPPanelSmoke+RECT
    [void][QPPanelSmoke]::GetWindowRect($panel, [ref]$rect)
    $dpi = [QPPanelSmoke]::GetDpiForWindow($panel); if ($dpi -eq 0) { $dpi = 96 }
    $monitor = [QPPanelSmoke]::MonitorFromWindow($panel, 2)
    $info = New-Object QPPanelSmoke+MONITORINFO
    $info.cbSize = [uint32][Runtime.InteropServices.Marshal]::SizeOf($info)
    $workW = 1920; $workH = 1080
    if ([QPPanelSmoke]::GetMonitorInfo($monitor, [ref]$info)) {
        $workW = $info.rcWork.Right - $info.rcWork.Left
        $workH = $info.rcWork.Bottom - $info.rcWork.Top
    }
    $scaledW = [int][math]::Floor((520 * $dpi + 48) / 96)
    $scaledH = [int][math]::Floor((420 * $dpi + 48) / 96)
    $margin = [int][math]::Floor((16 * $dpi + 48) / 96)
    $expectW = [math]::Min($scaledW, [math]::Max(1, $workW - $margin))
    $expectH = [math]::Min($scaledH, [math]::Max(1, $workH - $margin))
    Check "panel size matches DPI" (($rect.Right - $rect.Left) -eq $expectW -and ($rect.Bottom - $rect.Top) -eq $expectH) ("$($rect.Right - $rect.Left)x$($rect.Bottom - $rect.Top) dpi=$dpi expected=${expectW}x${expectH}")
}

# 3b. search filters the list, then clearing restores it
$edit = Find-Edit $panel
$openLines = @()
if (Test-Path $log) { $openLines = Get-Content $log -Encoding UTF8 | Select-Object -Skip $before }
$openMatch = [regex]::Matches(($openLines -join "`n"), "panel filter: matches=(\d+)")
$fullCount = if ($openMatch.Count -gt 0) { [int]$openMatch[$openMatch.Count - 1].Groups[1].Value } else { -1 }
$list = Find-List $panel
$fullRows = Get-ListCount $list
Check "panel starts with seeded items" ($fullRows -gt 0 -and $fullRows -eq $fullCount) ("rows=$fullRows log=$fullCount")

function Set-PanelSearch([IntPtr]$Target, [string]$Text) {
    $result = [IntPtr]::Zero
    return ([QPPanelSmoke]::SendMessageTimeout($Target, 0x000C, [IntPtr]::Zero, $Text, 0x0002, 2000, [ref]$result) -ne [IntPtr]::Zero)
}

$beforeSearch = 0
if (Test-Path $log) { $beforeSearch = (Get-Content $log -Encoding UTF8).Count }
Check "search text applied" (Set-PanelSearch $edit "json") ""
[void](Wait-Until { (Get-Content $log -Encoding UTF8 -ErrorAction SilentlyContinue | Select-Object -Skip $beforeSearch) -join "`n" -match "panel filter: matches=" } 1500)
$searchLines = @()
if (Test-Path $log) { $searchLines = Get-Content $log -Encoding UTF8 | Select-Object -Skip $beforeSearch }
$searchText = $searchLines -join "`n"
$match = [regex]::Matches($searchText, "panel filter: matches=(\d+)")
$searchCount = if ($match.Count -gt 0) { [int]$match[$match.Count - 1].Groups[1].Value } else { -1 }
$searchRows = Get-ListCount $list
Check "search filters results" ($searchCount -eq 1 -and $searchRows -eq 1) ("matches=$searchCount rows=$searchRows")
$beforeClear = 0
if (Test-Path $log) { $beforeClear = (Get-Content $log -Encoding UTF8).Count }
Check "search clear applied" (Set-PanelSearch $edit "") ""
[void](Wait-Until { $clear = Get-Content $log -Encoding UTF8 -ErrorAction SilentlyContinue | Select-Object -Skip $beforeClear; ($clear -join "`n") -match "panel filter: matches=" } 1500)
$clearLines = @()
if (Test-Path $log) { $clearLines = Get-Content $log -Encoding UTF8 | Select-Object -Skip $beforeClear }
$clearMatch = [regex]::Matches(($clearLines -join "`n"), "panel filter: matches=(\d+)")
$clearCount = if ($clearMatch.Count -gt 0) { [int]$clearMatch[$clearMatch.Count - 1].Groups[1].Value } else { -1 }
$clearRows = Get-ListCount $list
Check "clearing search restores all items" ($clearCount -eq $fullCount -and $clearRows -eq $fullRows) ("matches=$clearCount rows=$clearRows expected=$fullCount")

# 4. Esc hides
[void][QPPanelSmoke]::PostMessage($edit, 0x0100, [IntPtr]0x1B, [IntPtr]::Zero)
[void](Wait-Until { (-not [QPPanelSmoke]::IsWindowVisible($panel)) -and ([QPPanelSmoke]::GetForegroundWindow() -eq $restoreTarget) } 1500)
Find-Windows
Check "Esc hides panel" (-not [QPPanelSmoke]::IsWindowVisible($panel)) ""
Check "Esc restores foreground" ([QPPanelSmoke]::GetForegroundWindow() -eq $restoreTarget) ""

# 5. Enter commits the selected item and hides
$reopenMarker = 0
if (Test-Path $log) { $reopenMarker = (Get-Content $log -Encoding UTF8).Count }
$opened = Open-Panel 3
Check "panel reopens" $opened ""
[void](Wait-Until { (Get-Content $log -Encoding UTF8 -ErrorAction SilentlyContinue | Select-Object -Skip $reopenMarker) -join "`n" -match "panel shown in" } 1500)
$edit = Find-Edit $panel
[void][QPPanelSmoke]::PostMessage($edit, 0x0100, [IntPtr]0x0D, [IntPtr]::Zero)
[void][QPPanelSmoke]::PostMessage($edit, 0x0101, [IntPtr]0x0D, [IntPtr]::Zero)
[void](Wait-Until { (-not [QPPanelSmoke]::IsWindowVisible($panel)) -and ([QPPanelSmoke]::GetForegroundWindow() -eq $restoreTarget) } 1500)
Find-Windows
$newLines = @()
if (Test-Path $log) { $newLines = Get-Content $log -Encoding UTF8 | Select-Object -Skip $before }
$newText = $newLines -join "`n"
Check "Enter hides panel" (-not [QPPanelSmoke]::IsWindowVisible($panel)) ""
# Recent-use sorting means the first visible row is not necessarily data index 0;
# assert that Enter committed the selected row.
Check "commit logged a panel item" ($newText -match "panel (copy-only )?commit: index=\d+ ") ""
Check "Enter restores foreground" ([QPPanelSmoke]::GetForegroundWindow() -eq $restoreTarget) ""

# 6. close app
[void][QPPanelSmoke]::PostMessage($main, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
[void](Wait-Until { $app.HasExited } 3000)
Check "app exits 0" ($app.HasExited -and $app.ExitCode -eq 0) ("exit=" + $(if ($app.HasExited) { $app.ExitCode } else { "n/a" }))

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
} finally {
    Stop-QPTestRun
}
