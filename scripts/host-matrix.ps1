# M0-5 cross-process injection test.
# A separate process (target\host-probe.exe) owns a plain Win32 EDIT. QuickPhrase
# must capture it, show the panel, and inject the first hard-coded phrase into it.
#
# Usage: powershell -ExecutionPolicy Bypass -File scripts\host-matrix.ps1 -IsolatedDesktop [-SkipBuild]

param([switch]$SkipBuild, [switch]$IsolatedDesktop)

$ErrorActionPreference = "Stop"
if (-not $IsolatedDesktop) { throw "Run only on an isolated interactive desktop, with -IsolatedDesktop. Clipboard contents may change." }
. (Join-Path $PSScriptRoot "smoke-common.ps1")
Assert-QPTestIdle
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root "target\release\quickphrase.exe"

if (-not $SkipBuild) {
    Push-Location $root
    try {
        & cargo build --release
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    } finally { Pop-Location }
}
if (-not (Test-Path $exe)) { throw "missing $exe" }

$qpRun = New-QPTestRun $exe
$log = $qpRun.Log
$outDir = $qpRun.Directory
try {
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class QPHost {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowEx(IntPtr parent, IntPtr after, string cls, string title);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr SetActiveWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", EntryPoint="PostMessageW")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool attach);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
}
"@

$WM_HOTKEY = 0x0312
$WM_KEYDOWN = 0x0100
$WM_KEYUP = 0x0101
$VK_RETURN = 0x0D

$EXPECTED = "您好，目前该功能已完成开发，正在进行测试验证，预计测试通过后安排发布。"

$results = New-Object System.Collections.Generic.List[object]
function Check([string]$Name, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
}

function Find-WindowByClass([string]$Class) {
    $script:found = [IntPtr]::Zero
    $cb = [QPHost+EnumProc]{
        param($h, $unused)
        if (-not (Test-QPTestWindow $h)) { return $true }
        $sb = New-Object System.Text.StringBuilder 256
        [void][QPHost]::GetClassName($h, $sb, 256)
        if ($sb.ToString() -eq $Class) { $script:found = $h; return $false }
        return $true
    }
    [void][QPHost]::EnumWindows($cb, [IntPtr]::Zero)
    return $script:found
}

function Focus-Window([IntPtr]$Window) {
    for ($i = 0; $i -lt 8; $i++) {
        $fg = [QPHost]::GetForegroundWindow()
        if ($fg -eq $Window) { return $true }
        $fgPid = 0
        $fgThread = if ($fg -ne [IntPtr]::Zero) { [QPHost]::GetWindowThreadProcessId($fg, [ref]$fgPid) } else { 0 }
        $me = [QPHost]::GetCurrentThreadId()
        $attached = $false
        if ($fgThread -ne 0 -and $fgThread -ne $me) {
            $attached = [QPHost]::AttachThreadInput($me, $fgThread, $true)
        }
        [void][QPHost]::BringWindowToTop($Window)
        [void][QPHost]::SetActiveWindow($Window)
        [void][QPHost]::SetForegroundWindow($Window)
        if ($attached) { [void][QPHost]::AttachThreadInput($me, $fgThread, $false) }
        Start-Sleep -Milliseconds 250
    }
    return ([QPHost]::GetForegroundWindow() -eq $Window)
}

# The automation desktop refuses foreground handoff to the panel. Allow it to
# stay visible without focus; the probe window keeps the real focus, so the
# injection guards still run against a live target.

# --- start QuickPhrase ---------------------------------------------------
Start-Sleep -Milliseconds 400
$qp = Start-QPTest -AllowUnfocusedPanel
Start-Sleep -Milliseconds 1200
$main = Find-WindowByClass "QuickPhrase.MainWindow"
if ($main -eq [IntPtr]::Zero) { throw "QuickPhrase main window not found" }

# --- start the probe host ------------------------------------------------
$probe = Join-Path $root "target\host-probe.exe"
if (-not (Test-Path $probe)) { throw "missing $probe (build scripts\host-probe.rs first)" }

$hwndFile = Join-Path $outDir "host-hwnd.txt"
$textFile = Join-Path $outDir "host-text.txt"

$hostProcess = Start-QPTest -Probe $probe -Arguments @($textFile, $hwndFile)
for ($i = 0; $i -lt 40 -and -not (Test-Path $hwndFile); $i++) { Start-Sleep -Milliseconds 250 }
if (-not (Test-Path $hwndFile)) { throw "host probe did not publish its HWND" }
$hostWindow = [IntPtr][int64](Get-Content $hwndFile -Raw).Trim()
Check "host window created" ($hostWindow -ne [IntPtr]::Zero) ("hwnd=" + $hostWindow)

$focused = Focus-Window $hostWindow
Check "host focused" $focused ("fg=" + [QPHost]::GetForegroundWindow())
if (-not $focused) {
    "FAIL: cannot bring the host probe to the foreground (locked desktop or foreground lock)"
    exit 2
}
Start-Sleep -Milliseconds 500

# Observe the sequence without replacing the user's clipboard with a sentinel.
$clipboardBefore = [QPTestNative]::GetClipboardSequenceNumber()

$before = 0
if (Test-Path $log) { $before = (Get-Content $log).Count }

$opened = $false
$panel = [IntPtr]::Zero
[void](Focus-Window $hostWindow)
Start-Sleep -Milliseconds 300
[void][QPHost]::PostMessage($main, $WM_HOTKEY, [IntPtr]1, [IntPtr]::Zero)
for ($i = 0; $i -lt 20; $i++) {
    Start-Sleep -Milliseconds 200
    $candidate = Find-WindowByClass "QuickPhrase.Panel"
    if ($candidate -ne [IntPtr]::Zero -and [QPHost]::IsWindowVisible($candidate)) {
        $panel = $candidate
        $opened = $true
        break
    }
}
Check "panel opened" $opened ("hwnd=" + $panel)

$panelEdit = if ($panel -ne [IntPtr]::Zero) { [QPHost]::FindWindowEx($panel, [IntPtr]::Zero, "Edit", $null) } else { [IntPtr]::Zero }
Check "panel edit found" ($panelEdit -ne [IntPtr]::Zero) ""

if ($panelEdit -ne [IntPtr]::Zero) {
    [void][QPHost]::PostMessage($panelEdit, $WM_KEYDOWN, [IntPtr]$VK_RETURN, [IntPtr]::Zero)
    [void][QPHost]::PostMessage($panelEdit, $WM_KEYUP, [IntPtr]$VK_RETURN, [IntPtr]::Zero)
}
Start-Sleep -Milliseconds 2500

$textFile = Join-Path $outDir "host-text.txt"
$text = if (Test-Path $textFile) { [IO.File]::ReadAllText($textFile) } else { "<missing>" }
$newLines = @()
if (Test-Path $log) { $newLines = @(Get-Content $log -Encoding UTF8 | Select-Object -Skip $before) }
$newText = $newLines -join "`n"

Check "capture identity verified" ($newText -match "identity=verified") ""
Check "capture capability text" ($newText -match "capability=text") ""
Check "SendInput accepted" ($newText -match "inject accepted:") ""
Check "outcome reported" ($newText -match "inject outcome:") ""
Check "inserted text matches exactly" ($text -eq $EXPECTED) ("got=" + ($text -replace "`r", "\r" -replace "`n", "\n"))

$clipboardAfter = [QPTestNative]::GetClipboardSequenceNumber()
Check "clipboard untouched on accepted path" ($clipboardBefore -ne 0 -and $clipboardBefore -eq $clipboardAfter) "sequence=$clipboardBefore -> $clipboardAfter"

$failed = @($results | Where-Object { -not $_.Ok })
$results | Format-Table -AutoSize
"--- capture/inject log ---"
$newLines | Where-Object { $_ -match "capture:|inject|panel commit|position" } | ForEach-Object { $_ }
"--- host text ---"
($text -replace "`r", "\r" -replace "`n", "\n")

if ($failed.Count -gt 0) { "FAIL: $($failed.Count) check(s)"; exit 1 }
"PASS: $($results.Count) checks"
exit 0
} finally {
    Stop-QPTestRun
}
