# Builds the release zip with the executable, README and LICENSE.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package.ps1 [-SkipBuild]

param([switch]$SkipBuild)

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root "target\release\quickphrase.exe"

Push-Location $root
try {
    $metadata = & cargo metadata --no-deps --format-version 1 | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or -not $metadata.packages) { throw "cannot read package metadata" }
    $version = $metadata.packages[0].version
    if (-not $version) { throw "cannot read version" }

    if (-not $SkipBuild) {
        & cargo build --release
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    }
} finally {
    Pop-Location
}
if (-not (Test-Path $exe)) { throw "missing $exe" }

$versionInfo = (Get-Item $exe).VersionInfo
if ($versionInfo.FileVersion -ne $version) {
    throw "resource version '$($versionInfo.FileVersion)' does not match Cargo version '$version'"
}
if ($versionInfo.ProductVersion -ne $version) {
    throw "resource product version '$($versionInfo.ProductVersion)' does not match Cargo version '$version'"
}

$dist = Join-Path $root "dist"
$stage = Join-Path $dist "QuickPhrase-$version-win-x64"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Copy-Item $exe (Join-Path $stage "QuickPhrase.exe")
Copy-Item (Join-Path $root "README.md") $stage
Copy-Item (Join-Path $root "LICENSE") $stage

$zip = Join-Path $dist "QuickPhrase-$version-win-x64.zip"
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip -CompressionLevel Optimal

$zipSize = (Get-Item $zip).Length
if ($zipSize -gt 15MB) {
    Remove-Item $zip -Force -ErrorAction SilentlyContinue
    throw "package exceeds the 15 MB budget: $zipSize bytes"
}

$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
"$hash *QuickPhrase-$version-win-x64.zip" | Set-Content (Join-Path $dist "SHA256SUMS.txt") -Encoding ASCII

$exeSize = (Get-Item $exe).Length
"exe=$exe bytes=$exeSize"
"zip=$zip bytes=$zipSize"
"sha256=$hash"
"fileVersion=$($versionInfo.FileVersion) product=$($versionInfo.ProductName)"
"tag: git tag v$version && git push origin v$version"