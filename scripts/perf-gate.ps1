# Search/panel performance gate. Runs the release binary on a large corpus and
# records p50/p95/max with a hardware fingerprint. Thresholds are deliberately
# loose; run it on an idle machine and compare trends, not single runs.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\perf-gate.ps1 [-SkipBuild]

param([switch]$SkipBuild, [string]$TestedExeSha256)

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
$exeHash = (Get-FileHash $exe -Algorithm SHA256).Hash.ToLower()
if ($SkipBuild) {
    if (-not $TestedExeSha256) { throw "-SkipBuild requires -TestedExeSha256 from the tested build" }
    if ($exeHash -ne $TestedExeSha256.ToLower()) { throw "tested EXE hash does not match the current binary: $exeHash" }
} elseif ($TestedExeSha256 -and $exeHash -ne $TestedExeSha256.ToLower()) {
    throw "tested EXE hash does not match the rebuilt binary: $exeHash"
}
$version = (& cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).packages[0].version
$commit = (& git -C $root rev-parse --short HEAD)
if ($LASTEXITCODE -ne 0) { $commit = "unknown" }

$qpRun = New-QPTestRun $exe
$results = New-Object System.Collections.Generic.List[object]
function Check([string]$Name, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
}

function Get-Bench($Log, [int]$Before, [string]$Pattern) {
    $lines = @()
    if (Test-Path $Log) { $lines = Get-Content $Log -Encoding UTF8 | Select-Object -Skip $Before }
    $text = $lines -join "`n"
    return [regex]::Match($text, $Pattern)
}

try {
    # Large search corpus: 10k documents x 1k queries.
    $before = 0
    if (Test-Path $qpRun.Log) { $before = (Get-Content $qpRun.Log -Encoding UTF8).Count }
    $search = Start-QPTest -Arguments @("--search-bench", "--large") -Wait
    Check "search bench exits 0" ($search.ExitCode -eq 0) ("exit=" + $search.ExitCode)
    $match = Get-Bench $qpRun.Log $before "search bench: n=(\d+) queries=(\d+) p50=(\d+)us p95=(\d+)us max=(\d+)us"
    $p95 = if ($match.Success) { [int]$match.Groups[4].Value } else { -1 }
    $max = if ($match.Success) { [int]$match.Groups[5].Value } else { -1 }
    $documents = if ($match.Success) { [int]$match.Groups[1].Value } else { -1 }
    $queries = if ($match.Success) { [int]$match.Groups[2].Value } else { -1 }
    $searchP95 = $p95
    Check "large search reported" ($match.Success -and $documents -eq 10000 -and $queries -eq 1000) ("n=$documents queries=$queries")
    Check "large search p95 <= 200 ms" ($p95 -ge 0 -and $p95 -le 200000) ("p95=$p95 us max=$max us")

    # Panel filter/rank keep the existing 150 ms budget.
    $before = 0
    if (Test-Path $qpRun.Log) { $before = (Get-Content $qpRun.Log -Encoding UTF8).Count }
    $panel = Start-QPTest -Arguments "--panel-bench" -Wait
    Check "panel bench exits 0" ($panel.ExitCode -eq 0) ("exit=" + $panel.ExitCode)
    $match = Get-Bench $qpRun.Log $before "panel bench: n=100 failures=(\d+) p50=(\d+)us p95=(\d+)us max=(\d+)us"
    $panelFailures = if ($match.Success) { [int]$match.Groups[1].Value } else { -1 }
    $p95 = if ($match.Success) { [int]$match.Groups[3].Value } else { -1 }
    $panelP95 = $p95
    Check "panel bench rendered every frame" ($panelFailures -eq 0) ("failures=$panelFailures")
    Check "panel bench p95 <= 150 ms" ($p95 -gt 0 -and $p95 -le 150000) ("p95=$p95 us")

    $cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name)
    $os = [Environment]::OSVersion.VersionString
    $date = (Get-Date).ToString("s")
    $report = Join-Path (Join-Path $root "dist") ("perf-" + (Get-Date -Format "yyyyMMdd-HHmmss") + ".txt")
    New-Item -ItemType Directory -Force -Path (Split-Path $report -Parent) | Out-Null
    @(
        "date=$date",
        "cpu=$cpu",
        "os=$os",
        "version=$version",
        "commit=$commit",
        "exe_sha256=$exeHash",
        "search_documents=10000",
        "search_queries=1000",
        "panel_documents=50",
        "search_p95_us=$searchP95",
        "panel_p95_us=$panelP95",
        "results:",
        ($results | Format-Table -AutoSize | Out-String)
    ) | Set-Content $report -Encoding UTF8
    "report=$report"

    $results | Format-Table -AutoSize
    $failed = @($results | Where-Object { -not $_.Ok })
    if ($failed.Count -gt 0) {
        "FAIL: $($failed.Count) check(s)"
        exit 1
    }
    "PASS: $($results.Count) checks"
    exit 0
} finally {
    Stop-QPTestRun
}