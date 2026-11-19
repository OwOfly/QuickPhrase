# M1-6 tray smoke test
# Usage: powershell -ExecutionPolicy Bypass -File scripts\tray-smoke.ps1 [-SkipBuild]

param([switch]$SkipBuild)

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root "target\release\quickphrase.exe"
$log = Join-Path $env:LOCALAPPDATA ("QuickPhrase\logs\qp-{0}.log" -f (Get-Date -Format "yyyy-MM-dd"))

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

function Find-Main {
    $script:found = [IntPtr]::Zero
    $cb = [QPTray+EnumProc]{
        param($h, $l)
        $sb = New-Object System.Text.StringBuilder 256
        [void][QPTray]::GetClassName($h, $sb, 256)
        if ($sb.ToString() -eq "QuickPhrase.MainWindow") {
            $script:found = $h
            return $false
        }
        return $true
    }
    [void][QPTray]::EnumWindows($cb, [IntPtr]::Zero)
    return $script:found
}

$before = 0
if (Test-Path $log) { $before = (Get-Content $log).Count }

$app = Start-Process -FilePath $exe -PassThru
Start-Sleep -Milliseconds 1500
$main = Find-Main
Check "app running" (-not $app.HasExited) ("pid=" + $app.Id)
Check "main window exists" ($main -ne [IntPtr]::Zero) ""

$lines = @()
if (Test-Path $log) { $lines = @(Get-Content $log -Encoding UTF8 | Select-Object -Skip $before) }
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
$lines = @(Get-Content $log -Encoding UTF8 | Select-Object -Skip $before)
$text = $lines -join "`n"
Check "pause hotkey" ($text -match "hotkey paused") ""
Check "resume hotkey" ($text -match "hotkey resumed") ""

# Second instance signals the first and exits.
$second = Start-Process -FilePath $exe -PassThru -Wait
Start-Sleep -Milliseconds 600
Check "second instance exits 0" ($second.ExitCode -eq 0) ("exit=" + $second.ExitCode)
$lines = @(Get-Content $log -Encoding UTF8 | Select-Object -Skip $before)
$text = $lines -join "`n"
Check "second instance activation" ($text -match "second instance activation requested") ""

# Exit through the tray command.
[void][QPTray]::PostMessage($main, 0x0111, [IntPtr]1006, [IntPtr]::Zero)
Start-Sleep -Milliseconds 1200
$app.Refresh()
Check "app exits 0" ($app.HasExited -and $app.ExitCode -eq 0) ("exit=" + $(if ($app.HasExited) { $app.ExitCode } else { "running" }))
Check "hotkey released on exit" (Test-HotkeyFree) ""

$lines = @(Get-Content $log -Encoding UTF8 | Select-Object -Skip $before)
$text = $lines -join "`n"
Check "message loop ended" ($text -match "message loop ended") ""

if (-not $app.HasExited) { Stop-Process -Id $app.Id -Force }

$failed = @($results | Where-Object { -not $_.Ok })
$results | Format-Table -AutoSize
"--- tray log ---"
$lines | Where-Object { $_ -match "tray|hotkey|second instance|exit" } | ForEach-Object { $_ }

if ($failed.Count -gt 0) { "FAIL: $($failed.Count) check(s)"; exit 1 }
"PASS: $($results.Count) checks"
exit 0