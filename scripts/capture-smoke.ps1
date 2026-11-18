# M0-2 capture smoke test
# Usage: powershell -ExecutionPolicy Bypass -File scripts\capture-smoke.ps1 [-SkipBuild]

param([switch]$SkipBuild)

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root "target\release\quickphrase.exe"
$log = Join-Path $env:LOCALAPPDATA ("QuickPhrase\logs\qp-{0}.log" -f (Get-Date -Format yyyy-MM-dd))
$captures = Join-Path $env:LOCALAPPDATA "QuickPhrase\captures"

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

$results = New-Object System.Collections.Generic.List[object]
function Check([string]$Name, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
}

$before = 0
if (Test-Path $log) { $before = (Get-Content $log).Count }

$process = Start-Process -FilePath $exe -ArgumentList "--capture-test", "--delay", "600" -PassThru -Wait
Check "capture-test exit 0" ($process.ExitCode -eq 0) ("exit=" + $process.ExitCode)

$newLog = @()
if (Test-Path $log) { $newLog = Get-Content $log | Select-Object -Skip $before }
$logText = $newLog -join "`n"
Check "uia warmup ok" ($logText -match "uia warmup: true") ""
Check "capture written" ($logText -match "capture written:") ""
Check "compare_last executed" ($logText -match "compare_last=") ""

$report = $null
if (Test-Path $captures) {
    $newest = Get-ChildItem $captures -Filter "capture-*.json" |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
    if ($newest) {
        $report = Get-Content $newest.FullName -Raw | ConvertFrom-Json
        Check "report parsed" $true $newest.Name
    }
}
Check "report exists" ($null -ne $report) ""

if ($report) {
    Check "identity present" ($report.identity -in @("verified", "unknown")) $report.identity
    Check "capability present" ($report.capability -in @("text", "native_edit", "value_only", "readonly", "password", "none")) $report.capability
    Check "region present" ($report.region -in @("terminal", "text_editor", "unknown")) $report.region
    Check "timings recorded" ($null -ne $report.timings_ms.total) ("total=" + $report.timings_ms.total + "ms")
    Check "uia budget <= 200ms" ($report.timings_ms.uia -le 200) ("uia=" + $report.timings_ms.uia + "ms")
    if ($report.uia) {
        Check "uia shape" ($null -ne $report.uia.ancestor_verified -and $null -ne $report.uia.native_hwnd_zero) ("hwnd_zero=" + $report.uia.native_hwnd_zero + "; ancestor=" + $report.uia.ancestor_verified)
    } else {
        Check "uia optional (timeout allowed)" $true "uia=null"
    }
}

$failed = @($results | Where-Object { -not $_.Ok })
$results | Format-Table -AutoSize

"--- capture log ---"
$newLog

if ($failed.Count -gt 0) {
    "FAIL: $($failed.Count) check(s)"
    exit 1
}
"PASS: $($results.Count) checks"
exit 0
