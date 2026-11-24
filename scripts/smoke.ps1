# M0-1 smoke test
# Usage: powershell -ExecutionPolicy Bypass -File scripts\smoke.ps1 [-SkipBuild]

param([switch]$SkipBuild, [switch]$AllowHotkeySkip)

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
public class QPSmoke {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll", EntryPoint="PostMessageW")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
}
"@

function Get-MainWindow([int]$ProcessId) {
    $script:found = [IntPtr]::Zero
    $callback = [QPSmoke+EnumProc]{
        param($hwnd, $lparam)
        [uint32]$ownerPid = 0
        [void][QPSmoke]::GetWindowThreadProcessId($hwnd, [ref]$ownerPid)
        if ($ownerPid -eq $ProcessId) {
            $class = New-Object System.Text.StringBuilder 256
            [void][QPSmoke]::GetClassName($hwnd, $class, 256)
            if ($class.ToString() -eq "QuickPhrase.MainWindow") {
                $script:found = $hwnd
            }
        }
        return $true
    }
    [void][QPSmoke]::EnumWindows($callback, [IntPtr]::Zero)
    return $script:found
}

function Close-MainWindow([int]$ProcessId) {
    $hwnd = Get-MainWindow $ProcessId
    if ($hwnd -eq [IntPtr]::Zero) { return $false }
    [void][QPSmoke]::PostMessage($hwnd, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
    return $true
}

$before = 0
if (Test-Path $log) { $before = (Get-Content $log -Encoding UTF8).Count }

$results = New-Object System.Collections.Generic.List[object]
$skipped = 0
function Check([string]$Name, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
}

# 1. cold start
$times = @()
foreach ($i in 1..5) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-QPTest -Arguments '--bench' -Wait
    if ($p.ExitCode -ne 0) { throw "Cold start check failed: $($p.ExitCode)" }
    $sw.Stop()
    $times += $sw.ElapsedMilliseconds
}
$sorted = $times | Sort-Object
$median = $sorted[2]
Check "cold start median <= 1500 ms" ($median -le 1500) ("runs=" + ($times -join ",") + " ms; median=$median")

# 2. first instance
$first = Start-QPTest
Start-Sleep -Milliseconds 900
Check "first instance alive" (-not $first.HasExited) ("pid=" + $first.Id)
$workingSetMb = [math]::Round((Get-Process -Id $first.Id).WorkingSet64 / 1MB, 2)
Check "idle working set <= 35 MB (early sample)" ($workingSetMb -le 35) ("$workingSetMb MB at ~1 s")

# 2b. real hotkey trigger (best effort: UIPI may block input injection)
$hotkeyError = $null
$hotkeyTestable = $true
try {
    Add-Type -AssemblyName System.Windows.Forms
    [System.Windows.Forms.SendKeys]::SendWait("^%q")
    [void](Wait-Until {
        (Get-Content $log -Encoding UTF8 -ErrorAction SilentlyContinue | Select-Object -Skip $before) -join "`n" -match "hotkey fired"
    } 2000)
} catch {
    $hotkeyTestable = $false
    $hotkeyError = $_.Exception.Message
}

$hotkeyObserved = $false
if ($hotkeyTestable) {
    $hotkeyLines = @()
    if (Test-Path $log) { $hotkeyLines = Get-Content $log -Encoding UTF8 | Select-Object -Skip $before }
    $hotkeyObserved = ($hotkeyLines -join "`n") -match "hotkey fired"
}

if ($hotkeyObserved) {
    Check "hotkey fires" $true ""
} else {
    $script:skipped += 1
    $reason = if ($hotkeyError) { $hotkeyError } else { "no hotkey event observed; injection may be blocked" }
    $results.Add([pscustomobject]@{
        Name   = "hotkey fires"
        Ok     = $true
        Detail = "SKIP: $reason"
    })
}

# 3. second instance
$second = Start-QPTest -WakePid $first.Id -Wait
Start-Sleep -Milliseconds 300
Check "second instance exits 0" ($second.ExitCode -eq 0) ("exit=" + $second.ExitCode)
Check "first instance survives" (-not $first.HasExited) ("pid=" + $first.Id)

# 4. graceful close
$closed = Close-MainWindow $first.Id
Start-Sleep -Milliseconds 900
Check "WM_CLOSE closes main window" ($closed -and $first.HasExited) ("closed=$closed; alive=" + (-not $first.HasExited))

# 5. hotkey released
$again = Start-QPTest -Arguments "--selftest" -Wait
Check "hotkey released after exit" ($again.ExitCode -eq 0) ("selftest exit=" + $again.ExitCode)

# 6. log assertions (this run only)
$newLines = @()
if (Test-Path $log) { $newLines = Get-Content $log -Encoding UTF8 | Select-Object -Skip $before }
$newText = $newLines -join "`n"
Check "hotkey registered in log" ($newText -match "hotkey registered: Ctrl\+Alt\+Q") ""
Check "second instance signalled" ($newText -match "second instance activation requested") ""

$failed = @($results | Where-Object { -not $_.Ok })
$results | Format-Table -AutoSize

"--- log tail ---"
if (Test-Path $log) { Get-Content $log -Encoding UTF8 -Tail 12 }

if ($failed.Count -gt 0) {
    "FAIL: $($failed.Count) check(s)"
    exit 1
}
if ($skipped -gt 0 -and -not $AllowHotkeySkip) {
    "SKIP: hotkey trigger was not observed; run on an interactive desktop or pass -AllowHotkeySkip"
    exit 3
}
$executed = $results.Count - $skipped
if ($skipped -gt 0) {
    "PASS (HOTKEY SKIPPED, ALLOWED): $executed checks ($skipped skipped)"
    exit 4
}
"PASS: $executed checks"
exit 0
} finally {
    Stop-QPTestRun
}
