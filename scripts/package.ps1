# Builds the release zip with the executable, README and LICENSE.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\package.ps1 [-SkipBuild]

param([switch]$SkipBuild, [string]$TestedExeSha256, [switch]$AllowUnreleased)

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

$exeHash = (Get-FileHash $exe -Algorithm SHA256).Hash.ToLower()
if ($SkipBuild) {
    if (-not $TestedExeSha256) {
        throw "-SkipBuild requires -TestedExeSha256 from the tested build"
    }
    if ($exeHash -ne $TestedExeSha256.ToLower()) {
        throw "tested EXE hash does not match the current binary: $exeHash"
    }
} elseif ($TestedExeSha256 -and $exeHash -ne $TestedExeSha256.ToLower()) {
    throw "tested EXE hash does not match the rebuilt binary: $exeHash"
}

if (-not $AllowUnreleased) {
    $dirty = & git -C $root status --porcelain
    if ($LASTEXITCODE -ne 0) { throw "cannot read git status" }
    if ($dirty) { throw "working tree is dirty; commit and tag the release, or pass -AllowUnreleased for a test package" }
    $tags = @(& git -C $root tag --points-at HEAD)
    if ($LASTEXITCODE -ne 0 -or -not ($tags -contains "v$version")) {
        throw "HEAD is not tagged v$version; pass -AllowUnreleased for a test package"
    }
}

$notes = Join-Path $root "RELEASE_NOTES-v$version.md"
if (-not (Test-Path $notes)) { throw "missing release notes: $notes" }

$manifest = Get-Content (Join-Path $root "app.manifest") -Raw
$manifestMatch = [regex]::Match($manifest, 'assemblyIdentity version="([0-9.]+)"')
if (-not $manifestMatch.Success) { throw "cannot read assemblyIdentity version from app.manifest" }
if (-not $manifestMatch.Groups[1].Value.Equals("$version.0")) {
    throw "app.manifest version '$($manifestMatch.Groups[1].Value)' does not match Cargo version '$version'"
}

$versionInfo = (Get-Item $exe).VersionInfo
if ($versionInfo.FileVersion -ne $version) {
    throw "resource version '$($versionInfo.FileVersion)' does not match Cargo version '$version'"
}
if ($versionInfo.ProductVersion -ne $version) {
    throw "resource product version '$($versionInfo.ProductVersion)' does not match Cargo version '$version'"
}

$dist = Join-Path $root "dist"
$stage = Join-Path $dist "QuickPhrase-$version-win-x64"
if (Test-Path $stage) {
    Remove-Item $stage -Recurse -Force -ErrorAction Stop
}
if (Test-Path $stage) { throw "staging directory could not be cleared: $stage" }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

Copy-Item $exe (Join-Path $stage "QuickPhrase.exe")
$stageExe = Join-Path $stage "QuickPhrase.exe"
$stageHash = (Get-FileHash $stageExe -Algorithm SHA256).Hash.ToLower()
if ($stageHash -ne $exeHash) {
    throw "staged EXE changed after the tested-hash check: $stageHash"
}
Copy-Item (Join-Path $root "README.md") $stage
Copy-Item (Join-Path $root "LICENSE") $stage

$expectedNames = @("LICENSE", "QuickPhrase.exe", "README.md")
$stagedNames = @(Get-ChildItem $stage -File | Select-Object -ExpandProperty Name | Sort-Object)
$diff = Compare-Object -ReferenceObject ($expectedNames | Sort-Object) -DifferenceObject $stagedNames
if ($diff) { throw "unexpected staged files: $($diff | Out-String)" }

$zip = Join-Path $dist "QuickPhrase-$version-win-x64.zip"
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip -CompressionLevel Optimal

Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::OpenRead($zip)
try {
    $zipNames = @($archive.Entries | Select-Object -ExpandProperty FullName | Sort-Object)
    $entry = $archive.GetEntry("QuickPhrase.exe")
    if ($null -eq $entry) { throw "zip is missing QuickPhrase.exe" }
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $stream = $entry.Open()
        try {
            $zipExeHash = ([BitConverter]::ToString($sha.ComputeHash($stream))).Replace("-", "").ToLower()
        } finally {
            $stream.Dispose()
        }
    } finally {
        $sha.Dispose()
    }
    $stageHash = (Get-FileHash (Join-Path $stage "QuickPhrase.exe") -Algorithm SHA256).Hash.ToLower()
    if ($zipExeHash -ne $stageHash -or $zipExeHash -ne $exeHash) {
        Remove-Item $zip -Force -ErrorAction SilentlyContinue
        throw "zip EXE bytes differ from the tested EXE"
    }
} finally {
    $archive.Dispose()
}
$diff = Compare-Object -ReferenceObject ($expectedNames | Sort-Object) -DifferenceObject $zipNames
if ($diff) { throw "unexpected zip contents: $($diff | Out-String)" }

$zipSize = (Get-Item $zip).Length
if ($zipSize -gt 15MB) {
    Remove-Item $zip -Force -ErrorAction SilentlyContinue
    throw "package exceeds the 15 MB budget: $zipSize bytes"
}

$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
if ($AllowUnreleased) {
    "UNRELEASED PACKAGE: not tagged v$version; do not publish these artifacts"
}
$exeAsset = Join-Path $dist "QuickPhrase-$version-win-x64.exe"
Copy-Item (Join-Path $stage "QuickPhrase.exe") $exeAsset -Force
$exeHash = (Get-FileHash $exeAsset -Algorithm SHA256).Hash.ToLower()
"$hash *QuickPhrase-$version-win-x64.zip`n$exeHash *QuickPhrase-$version-win-x64.exe" |
    Set-Content (Join-Path $dist "SHA256SUMS.txt") -Encoding ASCII

$exeSize = (Get-Item $exe).Length
"exe=$exe bytes=$exeSize"
"exeAsset=$exeAsset bytes=$((Get-Item $exeAsset).Length)"
"zip=$zip bytes=$zipSize"
"sha256Zip=$hash"
"sha256Exe=$exeHash"
"fileVersion=$($versionInfo.FileVersion) product=$($versionInfo.ProductName)"
"tag: git tag v$version && git push origin v$version"
