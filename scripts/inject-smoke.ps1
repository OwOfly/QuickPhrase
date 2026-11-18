# M0-4 injection smoke test
# Usage: powershell -ExecutionPolicy Bypass -File scripts\inject-smoke.ps1 [-SkipBuild]

param([switch]$SkipBuild)

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root "target\release\quickphrase.exe"
$log = Join-Path $env:LOCALAPPDATA ("QuickPhrase\logs\qp-{0}.log" -f (Get-Date -Format yyyy-MM-dd))

# A resident instance locks the release binary, so stop it before building.
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

$results = New-Object System.Collections.Generic.List[object]
function Check([string]$Name, [bool]$Ok, [string]$Detail) {
    $results.Add([pscustomobject]@{ Name = $Name; Ok = $Ok; Detail = $Detail })
}

$before = 0
if (Test-Path $log) { $before = (Get-Content $log).Count }

$process = Start-Process -FilePath $exe -ArgumentList "--inject-test" -PassThru -Wait
Check "inject-test exit 0" ($process.ExitCode -eq 0) ("exit=" + $process.ExitCode)

$newLines = @()
if (Test-Path $log) { $newLines = Get-Content $log -Encoding UTF8 | Select-Object -Skip $before }
$newText = $newLines -join "`n"

Check "SendInput accepted" ($newText -match "inject accepted:") ""
Check "text read back" ($newText -match "Injected" -and $newText -match "text=") ""
Check "selftest passed" ($newText -match "inject-test: PASS") ""

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