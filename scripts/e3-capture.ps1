# M0-5 experiment E3: capture latency distribution for a live host.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\e3-capture.ps1 [-Runs 30]

param([int]$Runs = 30)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "smoke-common.ps1")
Assert-QPTestIdle
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root "target\release\quickphrase.exe"
$probe = Join-Path $root "target\host-probe.exe"

if (-not (Test-Path $exe)) { throw "missing $exe" }
if (-not (Test-Path $probe)) { throw "missing $probe (build scripts\host-probe.rs first)" }

$qpRun = New-QPTestRun $exe
$log = $qpRun.Log
$outDir = $qpRun.Directory
try {
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class QPE3 {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr SetActiveWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool attach);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
}
"@

function Focus-Window([IntPtr]$Window) {
    for ($i = 0; $i -lt 8; $i++) {
        if ([QPE3]::GetForegroundWindow() -eq $Window) { return $true }
        $fg = [QPE3]::GetForegroundWindow()
        $fgPid = 0
        $fgThread = if ($fg -ne [IntPtr]::Zero) { [QPE3]::GetWindowThreadProcessId($fg, [ref]$fgPid) } else { 0 }
        $me = [QPE3]::GetCurrentThreadId()
        $attached = $false
        if ($fgThread -ne 0 -and $fgThread -ne $me) { $attached = [QPE3]::AttachThreadInput($me, $fgThread, $true) }
        [void][QPE3]::BringWindowToTop($Window)
        [void][QPE3]::SetActiveWindow($Window)
        [void][QPE3]::SetForegroundWindow($Window)
        if ($attached) { [void][QPE3]::AttachThreadInput($me, $fgThread, $false) }
        Start-Sleep -Milliseconds 200
    }
    return ([QPE3]::GetForegroundWindow() -eq $Window)
}

$hwndFile = Join-Path $outDir "host-hwnd.txt"
$textFile = Join-Path $outDir "host-text.txt"

$hostProcess = Start-QPTest -Probe $probe -Arguments @($textFile, $hwndFile)
for ($i = 0; $i -lt 40 -and -not (Test-Path $hwndFile); $i++) { Start-Sleep -Milliseconds 250 }
if (-not (Test-Path $hwndFile)) { throw "host probe did not start" }
$hostWindow = [IntPtr][int64](Get-Content $hwndFile -Raw).Trim()

$samples = New-Object System.Collections.Generic.List[int]
$identity = 0
$region = 0
$before = 0
if (Test-Path $log) { $before = (Get-Content $log).Count }

for ($i = 0; $i -lt $Runs; $i++) {
    if (-not (Focus-Window $hostWindow)) { throw "Cannot focus the host probe" }
    Start-Sleep -Milliseconds 120
    $sample = Start-QPTest -Arguments @("--capture-test", "--delay", "120") -Wait
    if ($sample.ExitCode -ne 0) { throw "Capture probe failed: $($sample.ExitCode)" }
}

$lines = @()
if (Test-Path $log) { $lines = @(Get-Content $log -Encoding UTF8 | Select-Object -Skip $before) }
foreach ($line in $lines) {
    if ($line -match "capture-test: identity=(\w+) source=(\w+) capability=(\w+) region=(\w+).*uia=(\d+)ms total=(\d+)ms") {
        $samples.Add([int]$Matches[5])
        if ($Matches[1] -eq "verified") { $identity++ }
        if ($Matches[4] -eq "text_editor") { $region++ }
    }
}

if ($samples.Count -eq 0) { "FAIL: no capture samples"; exit 1 }
$sorted = $samples | Sort-Object
$p50 = $sorted[[int][Math]::Floor($sorted.Count * 0.5)]
$p95 = $sorted[[Math]::Min($sorted.Count - 1, [int][Math]::Floor($sorted.Count * 0.95))]
$max = $sorted[$sorted.Count - 1]

"E3 capture samples: n=$($samples.Count) min=$($sorted[0])ms p50=${p50}ms p95=${p95}ms max=${max}ms"
"identity=verified: $identity / $($samples.Count)"
"region=text_editor: $region / $($samples.Count)"
exit 0
} finally {
    Stop-QPTestRun
}
