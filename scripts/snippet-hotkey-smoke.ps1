# Per-snippet hotkey smoke test.
# Uses isolated portable data, posts WM_HOTKEY for the one snippet hotkey and
# asserts that the direct injection reached the separate host process exactly.
#
# Usage: powershell -ExecutionPolicy Bypass -File scripts\snippet-hotkey-smoke.ps1 -IsolatedDesktop [-SkipBuild]

param([switch]$SkipBuild, [switch]$IsolatedDesktop)

$ErrorActionPreference = "Stop"
if (-not $IsolatedDesktop) { throw "Run only on an isolated interactive desktop, with -IsolatedDesktop. Clipboard contents may change." }
. (Join-Path $PSScriptRoot "smoke-common.ps1")
Assert-QPTestIdle
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root "target\release\quickphrase.exe"
$probe = Join-Path $root "target\host-probe.exe"
$expected = "快捷键直出内容"
$hotkey = "Ctrl+Shift+F9"

if (-not $SkipBuild) {
    Push-Location $root
    try {
        & cargo build --release
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    } finally { Pop-Location }
}
if (-not (Test-Path $exe)) { throw "missing $exe" }
if (-not (Test-Path $probe)) { throw "missing $probe (build scripts\host-probe.rs first)" }

$qpRun = New-QPTestRun $exe
$log = $qpRun.Log
$outDir = $qpRun.Directory
try {
$dataDir = $qpRun.Data
$data = @{
    schemaVersion = 1
    revision = 0
    categories = @()
    snippets = @(
        @{
            id = "hotkey-smoke"
            title = "快捷键烟测"
            content = $expected
            categoryId = $null
            pinned = $false
            sortOrder = 0
            createdAt = "2026-11-22T00:00:00Z"
            updatedAt = "2026-11-22T00:00:00Z"
            useCount = 0
            lastUsedAt = $null
            lastInsertStatus = $null
            deletedAt = $null
            tags = @()
            hotkey = $hotkey
        }
    )
}
$json = $data | ConvertTo-Json -Depth 8
[IO.File]::WriteAllText((Join-Path $dataDir "data.json"), $json, (New-Object Text.UTF8Encoding($false)))

Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class QPSnippetHotkey {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
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
$results = New-Object System.Collections.Generic.List[object]
function Check([string]$Name, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
}

function Find-WindowByClass([string]$Class) {
    $script:found = [IntPtr]::Zero
    $cb = [QPSnippetHotkey+EnumProc]{
        param($h, $unused)
        if (-not (Test-QPTestWindow $h)) { return $true }
        $sb = New-Object System.Text.StringBuilder 256
        [void][QPSnippetHotkey]::GetClassName($h, $sb, 256)
        if ($sb.ToString() -eq $Class) { $script:found = $h; return $false }
        return $true
    }
    [void][QPSnippetHotkey]::EnumWindows($cb, [IntPtr]::Zero)
    return $script:found
}

function Focus-Window([IntPtr]$Window) {
    for ($i = 0; $i -lt 8; $i++) {
        $fg = [QPSnippetHotkey]::GetForegroundWindow()
        if ($fg -eq $Window) { return $true }
        $fgPid = 0
        $fgThread = if ($fg -ne [IntPtr]::Zero) { [QPSnippetHotkey]::GetWindowThreadProcessId($fg, [ref]$fgPid) } else { 0 }
        $me = [QPSnippetHotkey]::GetCurrentThreadId()
        $attached = $false
        if ($fgThread -ne 0 -and $fgThread -ne $me) {
            $attached = [QPSnippetHotkey]::AttachThreadInput($me, $fgThread, $true)
        }
        [void][QPSnippetHotkey]::BringWindowToTop($Window)
        [void][QPSnippetHotkey]::SetActiveWindow($Window)
        [void][QPSnippetHotkey]::SetForegroundWindow($Window)
        if ($attached) { [void][QPSnippetHotkey]::AttachThreadInput($me, $fgThread, $false) }
        Start-Sleep -Milliseconds 250
    }
    return ([QPSnippetHotkey]::GetForegroundWindow() -eq $Window)
}

$qp = Start-QPTest
Start-Sleep -Milliseconds 1500
$main = Find-WindowByClass "QuickPhrase.MainWindow"
Check "main window found" ($main -ne [IntPtr]::Zero) ("hwnd=" + $main)
if ($main -eq [IntPtr]::Zero) { throw "QuickPhrase main window not found" }

$hwndFile = Join-Path $outDir "snippet-hotkey-hwnd.txt"
$textFile = Join-Path $outDir "snippet-hotkey-text.txt"
$hostProcess = Start-QPTest -Probe $probe -Arguments @($textFile, $hwndFile)
for ($i = 0; $i -lt 40 -and -not (Test-Path $hwndFile); $i++) { Start-Sleep -Milliseconds 250 }
if (-not (Test-Path $hwndFile)) { throw "host probe did not publish its HWND" }
$hostWindow = [IntPtr][int64](Get-Content $hwndFile -Raw).Trim()
Check "host window created" ($hostWindow -ne [IntPtr]::Zero) ("hwnd=" + $hostWindow)

$focused = Focus-Window $hostWindow
Check "host focused" $focused ("fg=" + [QPSnippetHotkey]::GetForegroundWindow())
if (-not $focused) {
    "FAIL: cannot bring the host probe to the foreground"
    exit 2
}
Start-Sleep -Milliseconds 400

# Observe the sequence without replacing the user's clipboard with a sentinel.
$clipboardBefore = [QPTestNative]::GetClipboardSequenceNumber()

$before = 0
if (Test-Path $log) { $before = @(Get-Content $log -Encoding UTF8).Count }

[void][QPSnippetHotkey]::PostMessage($main, $WM_HOTKEY, [IntPtr]1000, [IntPtr]::Zero)
$text = ""
for ($i = 0; $i -lt 30; $i++) {
    Start-Sleep -Milliseconds 200
    if (Test-Path $textFile) {
        $text = [IO.File]::ReadAllText($textFile)
        if ($text -eq $expected) { break }
    }
}

$newLines = @()
if (Test-Path $log) { $newLines = @(Get-Content $log -Encoding UTF8 | Select-Object -Skip $before) }
$newText = $newLines -join "`n"
$allText = if (Test-Path $log) { (Get-Content $log -Encoding UTF8) -join "`n" } else { "" }

Check "capture identity verified" ($newText -match "identity=verified") ""
Check "capture capability text" ($newText -match "capability=text") ""
Check "SendInput accepted" ($newText -match "inject accepted:") ""
Check "snippet hotkey outcome reported" ($newText -match "snippet hotkey inject outcome:") ""
Check "inserted text matches exactly" ($text -eq $expected) ("got=" + ($text -replace "`r", "\r" -replace "`n", "\n"))
Check "snippet hotkey registered at startup" ($allText -match "snippet hotkey registered id=1000") ""

$panel = Find-WindowByClass "QuickPhrase.Panel"
$panelVisible = $panel -ne [IntPtr]::Zero -and [QPSnippetHotkey]::IsWindowVisible($panel)
Check "panel not opened" (-not $panelVisible) ("visible=" + $panelVisible)

$clipboardAfter = [QPTestNative]::GetClipboardSequenceNumber()
Check "clipboard untouched on accepted path" ($clipboardBefore -ne 0 -and $clipboardBefore -eq $clipboardAfter) "sequence=$clipboardBefore -> $clipboardAfter"

# --- self-window guard: pressing the snippet hotkey inside QuickPhrase must
# not copy, inject, or bump useCount.
[void][QPSnippetHotkey]::PostMessage($main, 0x0111, [IntPtr]1002, [IntPtr]::Zero)
$manage = [IntPtr]::Zero
for ($i = 0; $i -lt 20; $i++) {
    Start-Sleep -Milliseconds 200
    $candidate = Find-WindowByClass "QuickPhrase.ManageWindow"
    if ($candidate -ne [IntPtr]::Zero -and [QPSnippetHotkey]::IsWindowVisible($candidate)) {
        $manage = $candidate
        break
    }
}
Check "manage window opened for self test" ($manage -ne [IntPtr]::Zero) ("hwnd=" + $manage)
$selfFocused = $false
if ($manage -ne [IntPtr]::Zero) {
    $selfFocused = Focus-Window $manage
}
Check "manage focused for self test" $selfFocused ("fg=" + [QPSnippetHotkey]::GetForegroundWindow())

if ($manage -ne [IntPtr]::Zero -and $selfFocused) {
    $dataPath = Join-Path $dataDir "data.json"
    $beforeData = [IO.File]::ReadAllText($dataPath, [Text.Encoding]::UTF8) | ConvertFrom-Json
    $beforeUseCount = [int64]$beforeData.snippets[0].useCount
    $selfClipboardBefore = [QPTestNative]::GetClipboardSequenceNumber()
    $selfBefore = 0
    if (Test-Path $log) { $selfBefore = @(Get-Content $log -Encoding UTF8).Count }
    [void][QPSnippetHotkey]::PostMessage($main, $WM_HOTKEY, [IntPtr]1000, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 1200
    $selfClip = [QPTestNative]::GetClipboardSequenceNumber()
    $afterData = [IO.File]::ReadAllText($dataPath, [Text.Encoding]::UTF8) | ConvertFrom-Json
    $afterUseCount = [int64]$afterData.snippets[0].useCount
    $selfLines = if (Test-Path $log) { (@(Get-Content $log -Encoding UTF8 | Select-Object -Skip $selfBefore)) -join "`n" } else { "" }
    Check "self hotkey ignored" ($selfLines -match "foreground is QuickPhrase itself") ""
    Check "self clipboard untouched" ($selfClipboardBefore -ne 0 -and $selfClip -eq $selfClipboardBefore) "sequence=$selfClipboardBefore -> $selfClip"
    Check "self useCount unchanged" ($afterUseCount -eq $beforeUseCount) ("before=$beforeUseCount after=$afterUseCount")
} else {
    Check "self hotkey ignored" $false "manage window unavailable or unfocused"
    Check "self clipboard untouched" $false "manage window unavailable or unfocused"
    Check "self useCount unchanged" $false "manage window unavailable or unfocused"
}

$failed = @($results | Where-Object { -not $_.Ok })
$results | Format-Table -AutoSize

if ($failed.Count -gt 0) { "FAIL: $($failed.Count) check(s)"; exit 1 }
"PASS: $($results.Count) checks"
exit 0
} finally {
    Stop-QPTestRun
}
