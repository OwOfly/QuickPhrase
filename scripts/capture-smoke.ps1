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
$startedAt = Get-Date

$process = Start-Process -FilePath $exe -ArgumentList "--capture-test", "--delay", "600" -PassThru -Wait
Check "capture-test exit 0" ($process.ExitCode -eq 0) ("exit=" + $process.ExitCode)

$newLog = @()
if (Test-Path $log) { $newLog = Get-Content $log | Select-Object -Skip $before }
$logText = $newLog -join "`n"

function Exit-Skip([string]$Reason) {
    $failed = @($results | Where-Object { -not $_.Ok })
    $results | Format-Table -AutoSize
    if ($failed.Count -gt 0) {
        "FAIL: $($failed.Count) check(s)"
        exit 1
    }
    "SKIP: $Reason"
    exit 3
}

if ($logText -match "no foreground window") {
    Exit-Skip "no foreground window in this desktop session"
}

Check "uia warmup ok" ($logText -match "uia warmup: true") ""
Check "capture written" ($logText -match "capture written:") ""
Check "compare_last true" ($logText -match "compare_last=Some\(true\)") ""

$report = $null
if (Test-Path $captures) {
    $newest = Get-ChildItem $captures -Filter "capture-*.json" |
        Where-Object { $_.LastWriteTime -ge $startedAt.AddSeconds(-2) } |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
    if (-not $newest) {
        if ($logText -notmatch "capture written:") {
            Check "capture written missing" $false ""
        }
        Exit-Skip "no fresh capture report"
    }
    if ($newest) {
        $report = Get-Content $newest.FullName -Raw -Encoding UTF8 | ConvertFrom-Json
        Check "report parsed" $true $newest.Name
    }
}
Check "report exists" ($null -ne $report) ""

if ($report) {
    Check "identity verified" ($report.identity -eq "verified") $report.identity
    Check "capability known" ($report.capability -in @("text", "value_only", "readonly", "disabled", "password", "elevated", "integrity_unknown", "no_caret", "document", "unknown", "uia_unavailable")) $report.capability
    Check "region known" ($report.region -in @("terminal", "text_editor", "unknown")) $report.region
    Check "timings recorded" ($null -ne $report.timings_ms.total) ("total=" + $report.timings_ms.total + "ms")
    Check "uia budget <= 60ms" ($report.timings_ms.uia -le 60) ("uia=" + $report.timings_ms.uia + "ms")
    Check "uia present" ($null -ne $report.uia) ""
    if ($report.uia) {
        Check "uia ancestor verified" ($report.uia.ancestor_verified -eq $true) ("depth=" + $report.uia.ancestor_depth)
        Check "uia foreground stable" ($report.uia.foreground_stable -eq $true) ""
        Check "uia elapsed <= 60ms" ($report.uia.elapsed_ms -le 60) ("uia_elapsed=" + $report.uia.elapsed_ms + "ms")
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
