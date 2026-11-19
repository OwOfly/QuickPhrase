# Builds the release zip with the executable, README and LICENSE.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package.ps1 [-SkipBuild]

param([switch]$SkipBuild)

$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $root "target\release\quickphrase.exe"

$version = (Select-String -Path (Join-Path $root "Cargo.toml") -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $version) { throw "cannot read version from Cargo.toml" }

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

$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
"$hash *QuickPhrase-$version-win-x64.zip" | Set-Content (Join-Path $dist "SHA256SUMS.txt") -Encoding ASCII

$zipSize = (Get-Item $zip).Length
$exeSize = (Get-Item $exe).Length
$versionInfo = (Get-Item $exe).VersionInfo
if ($zipSize -gt 15MB) { throw "package exceeds the 15 MB budget: $zipSize bytes" }

"exe=$exe bytes=$exeSize"
"zip=$zip bytes=$zipSize"
"sha256=$hash"
"fileVersion=$($versionInfo.FileVersion) product=$($versionInfo.ProductName)"
"tag: git tag v$version && git push origin v$version"