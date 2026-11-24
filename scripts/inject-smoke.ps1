# M0-4 injection smoke test
# Usage: powershell -ExecutionPolicy Bypass -File scripts\inject-smoke.ps1 -IsolatedDesktop [-SkipBuild]

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
    } finally {
        Pop-Location
    }
}
if (-not (Test-Path $exe)) { throw "missing $exe" }

$qpRun = New-QPTestRun $exe
$log = $qpRun.Log
try {
$results = New-Object System.Collections.Generic.List[object]
function Check([string]$Name, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
}

$before = 0
if (Test-Path $log) { $before = (Get-Content $log).Count }

$process = Start-QPTest -Arguments @("--inject-test", "--with-paste") -Wait
Check "inject-test exit 0" ($process.ExitCode -eq 0) ("exit=" + $process.ExitCode)

$newLines = @()
if (Test-Path $log) { $newLines = Get-Content $log -Encoding UTF8 | Select-Object -Skip $before }
$newText = $newLines -join "`n"

Check "SendInput accepted" ($newText -match "inject accepted:") ""
Check "text read back" ($newText -match "Injected" -and $newText -match "text=") ""
Check "selftest passed" ($newText -match "inject-test: PASS") ""
Check "paste reaches the original edit" ($newText -match "paste: normal=true") ""
Check "paste rejects focus moved within the same window" ($newText -match "paste: focus_change_rejected=true") ""

$failed = @($results | Where-Object { -not $_.Ok })
$results | Format-Table -AutoSize

"--- inject log ---"
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
