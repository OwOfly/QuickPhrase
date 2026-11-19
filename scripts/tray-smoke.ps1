# M1-6 tray smoke test
# Usage: powershell -ExecutionPolicy Bypass -File scripts\tray-smoke.ps1 [-SkipBuild]

param([switch]$SkipBuild)

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root "target\release\quickphrase.exe"
$log = Join-Path $env:LOCALAPPDATA ("QuickPhrase\logs\qp-{0}.log" -f (Get-Date -Format "yyyy-MM-dd"))

# The panel may open on a locked desktop; keep injection out of the smoke run.
$env:QUICKPHRASE_TEST_ALLOW_UNFOCUSED_PANEL = "1"
$env:QUICKPHRASE_TEST_NO_INJECT = "1"

Get-Process quickphrase -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 400

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
public class QPTray {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll", EntryPoint="PostMessageW")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool RegisterHotKey(IntPtr h, int id, uint modifiers, uint vk);
  [DllImport("user32.dll")] public static extern bool UnregisterHotKey(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
}
"@

$results = New-Object System.Collections.Generic.List[object]
function Check([string]$Name, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
}

# Ctrl+Alt+Q under a temporary id: true only when no other process holds it.
function Test-HotkeyFree {
    $id = 49211
    $free = [QPTray]::RegisterHotKey([IntPtr]::Zero, $id, 3, 0x51)
    if ($free) { [void][QPTray]::UnregisterHotKey([IntPtr]::Zero, $id) }
    return $free
}

function Find-Class([string]$Class) {
    $script:found = [IntPtr]::Zero
    $cb = [QPTray+EnumProc]{
        param($h, $l)
        $sb = New-Object System.Text.StringBuilder 256
        [void][QPTray]::GetClassName($h, $sb, 256)
        if ($sb.ToString() -eq $Class) {
            $script:found = $h
            return $false
        }
        return $true
    }
    [void][QPTray]::EnumWindows($cb, [IntPtr]::Zero)
    return $script:found
}

function Find-Main { return Find-Class "QuickPhrase.MainWindow" }

# Read only what the app appended since $offset bytes. A rotated/truncated file
# falls back to a full read instead of producing a false negative. The log is
# opened read/write by the app, so share both modes.
function Read-NewLog([long]$Offset, [string]$Path) {
    if (-not (Test-Path $Path)) { return @() }
    $stream = $null
    try {
        $stream = [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::ReadWrite)
        $length = $stream.Length
        $start = 0
        if ($length -gt $Offset) { $start = [int]$Offset }
        if ($length -le $start) { return @() }
        $count = [int]($length - $start)
        $bytes = New-Object byte[] $count
        [void]$stream.Seek($start, [IO.SeekOrigin]::Begin)
        $read = 0
        while ($read -lt $count) {
            $n = $stream.Read($bytes, $read, $count - $read)
            if ($n -le 0) { break }
            $read += $n
        }
        $text = [Text.Encoding]::UTF8.GetString($bytes, 0, $read)
        return @($text -split "`r?`n")
    } finally {
        if ($stream) { $stream.Dispose() }
    }
}

function Wait-Exit($Proc, [int]$TimeoutMs) {
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    while (-not $Proc.HasExited -and [DateTime]::UtcNow -lt $deadline) {
        Start-Sleep -Milliseconds 100
        $Proc.Refresh()
    }
    return $Proc.HasExited
}

# The panel may flash visible and lose activation on a locked desktop; poll
# briefly instead of sampling once.
function Wait-PanelVisible([int]$TimeoutMs) {
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    while ([DateTime]::UtcNow -lt $deadline) {
        $panel = Find-Class "QuickPhrase.Panel"
        if ($panel -ne [IntPtr]::Zero -and [QPTray]::IsWindowVisible($panel)) {
            return $panel
        }
        Start-Sleep -Milliseconds 50
    }
    return [IntPtr]::Zero
}

$before = 0
if (Test-Path $log) { $before = (Get-Item $log).Length }

$app = Start-Process -FilePath $exe -PassThru
Start-Sleep -Milliseconds 1500
$main = Find-Main
Check "app running" (-not $app.HasExited) ("pid=" + $app.Id)
Check "main window exists" ($main -ne [IntPtr]::Zero) ""

$lines = Read-NewLog $before $log
$text = $lines -join "`n"
Check "tray icon added" ($text -match "tray icon added") ""
Check "hotkey registered" ($text -match "hotkey registered:") ""
Check "hotkey is held while running" (-not (Test-HotkeyFree)) ""

# Pause / resume
[void][QPTray]::PostMessage($main, 0x0111, [IntPtr]1003, [IntPtr]::Zero)
Start-Sleep -Milliseconds 500
Check "pause releases the hotkey" (Test-HotkeyFree) ""
[void][QPTray]::PostMessage($main, 0x0111, [IntPtr]1003, [IntPtr]::Zero)
Start-Sleep -Milliseconds 500
Check "resume takes the hotkey again" (-not (Test-HotkeyFree)) ""
$lines = Read-NewLog $before $log
$text = $lines -join "`n"
Check "pause hotkey" ($text -match "hotkey paused") ""
Check "resume hotkey" ($text -match "hotkey resumed") ""

# Second instance opens the panel and exits; a second run must not close it.
$second = Start-Process -FilePath $exe -PassThru
$secondExited = Wait-Exit $second 10000
Check "second instance exits" $secondExited ("exit=" + $(if ($second.HasExited) { $second.ExitCode } else { "running" }))
Check "second instance exits 0" ($second.HasExited -and $second.ExitCode -eq 0) ("exit=" + $(if ($second.HasExited) { $second.ExitCode } else { "running" }))
if (-not $second.HasExited) { Stop-Process -Id $second.Id -Force }
Start-Sleep -Milliseconds 900
$lines = Read-NewLog $before $log
$text = $lines -join "`n"
Check "second instance activation" ($text -match "second instance activation requested") ""
$panel = Wait-PanelVisible 3000
Check "second instance opens the panel" ($panel -ne [IntPtr]::Zero) ""

# Ensure the panel is open before checking that a later instance leaves it open.
if ($panel -eq [IntPtr]::Zero -or -not [QPTray]::IsWindowVisible($panel)) {
    [void][QPTray]::PostMessage($main, 0x0111, [IntPtr]1001, [IntPtr]::Zero)
    $panel = Wait-PanelVisible 3000
}

$second2 = Start-Process -FilePath $exe -PassThru
$second2Exited = Wait-Exit $second2 10000
Check "second instance (panel open) exits 0" ($second2Exited -and $second2.ExitCode -eq 0) ("exit=" + $(if ($second2.HasExited) { $second2.ExitCode } else { "running" }))
if (-not $second2.HasExited) { Stop-Process -Id $second2.Id -Force }
Start-Sleep -Milliseconds 900
$panel = Find-Class "QuickPhrase.Panel"
Check "second instance keeps an open panel" ($panel -ne [IntPtr]::Zero -and [QPTray]::IsWindowVisible($panel)) ""

[void][QPTray]::PostMessage($panel, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds 600
Check "panel closes on WM_CLOSE" (-not [QPTray]::IsWindowVisible($panel)) ""

# Exit through the tray command.
[void][QPTray]::PostMessage($main, 0x0111, [IntPtr]1006, [IntPtr]::Zero)
Start-Sleep -Milliseconds 1200
$app.Refresh()
Check "app exits 0" ($app.HasExited -and $app.ExitCode -eq 0) ("exit=" + $(if ($app.HasExited) { $app.ExitCode } else { "running" }))
Check "hotkey released on exit" (Test-HotkeyFree) ""

$lines = Read-NewLog $before $log
$text = $lines -join "`n"
Check "message loop ended" ($text -match "message loop ended") ""

if (-not $app.HasExited) { Stop-Process -Id $app.Id -Force }

$failed = @($results | Where-Object { -not $_.Ok })
$results | Format-Table -AutoSize
"--- tray log ---"
$lines | Where-Object { $_ -match "tray|hotkey|second instance|exit" } | ForEach-Object { $_ }

Remove-Item Env:QUICKPHRASE_TEST_ALLOW_UNFOCUSED_PANEL, Env:QUICKPHRASE_TEST_NO_INJECT -ErrorAction SilentlyContinue

if ($failed.Count -gt 0) { "FAIL: $($failed.Count) check(s)"; exit 1 }
"PASS: $($results.Count) checks"
exit 0