# Pre-release checklist. Validates the repository state and prints the commands
# that must pass on an interactive desktop before publishing.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\release-checklist.ps1

param([switch]$Quiet)

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$results = New-Object System.Collections.Generic.List[object]
function Check([string]$Name, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
}

Push-Location $root
try {
    $metadata = & cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "cannot read cargo metadata" }
    $version = $metadata.packages[0].version
} finally {
    Pop-Location
}
if (-not $version) { throw "cannot read version" }

Check "release notes exist" (Test-Path (Join-Path $root "RELEASE_NOTES-v$version.md")) "RELEASE_NOTES-v$version.md"

$manifestPath = Join-Path $root "app.manifest"
$manifest = if (Test-Path $manifestPath) { Get-Content $manifestPath -Raw } else { "" }
$match = [regex]::Match($manifest, 'assemblyIdentity version="([0-9.]+)"')
Check "manifest version matches" ($match.Success -and $match.Groups[1].Value -eq "$version.0") $(if ($match.Success) { $match.Groups[1].Value } else { "missing" })

$dirty = @(& git -C $root status --porcelain)
Check "working tree clean" ($LASTEXITCODE -eq 0 -and $dirty.Count -eq 0) ($dirty -join "; ")
$tags = @(& git -C $root tag --points-at HEAD)
Check "HEAD tagged v$version" ($LASTEXITCODE -eq 0 -and ($tags -contains "v$version")) ($tags -join ", ")

$exe = Join-Path $root "target\release\quickphrase.exe"
$testedHash = if (Test-Path $exe) {
    (Get-FileHash $exe -Algorithm SHA256).Hash.ToLower()
} else {
    "<run cargo build --release first>"
}
$steps = @(
    "cargo build --release",
    "`$testedHash = (Get-FileHash target\release\quickphrase.exe -Algorithm SHA256).Hash.ToLower()   # $testedHash",
    "cargo fmt --check",
    "cargo clippy --release -- -D warnings",
    "cargo test",
    "scripts\smoke.ps1 -SkipBuild",
    "scripts\tray-smoke.ps1 -SkipBuild",
    "scripts\tag-batch-smoke.ps1 -SkipBuild",
    "scripts\panel-smoke.ps1 -SkipBuild   # interactive desktop",
    "rustc scripts\host-probe.rs -O -o target\host-probe.exe",
    "scripts\host-matrix.ps1 -SkipBuild -IsolatedDesktop   # interactive desktop",
    "scripts\snippet-hotkey-smoke.ps1 -SkipBuild -IsolatedDesktop   # interactive desktop",
    "scripts\inject-smoke.ps1 -SkipBuild -IsolatedDesktop   # interactive desktop",
    "scripts\perf-gate.ps1 -SkipBuild -TestedExeSha256 `$testedHash",
    "scripts\package.ps1 -TestedExeSha256 `$testedHash   # or -SkipBuild with the same hash",
    "exit/session gate: test-m4a.ps1 (review harness; portable runner still pending)",
    "SHA256SUMS.txt + release notes + zip/exe uploaded together"
)

$failed = @($results | Where-Object { -not $_.Ok })
if (-not $Quiet) {
    $results | Format-Table -AutoSize
    "Release commands:"
    foreach ($step in $steps) { "  - $step" }
}
if ($failed.Count -gt 0) {
    "FAIL: $($failed.Count) release check(s)"
    exit 1
}
if (-not $Quiet) { "PASS: $($results.Count) release state checks" }
exit 0