# Generates assets/quickphrase.ico from assets/quickphrase-logo.png.
# Usage: powershell -ExecutionPolicy Bypass -File scripts\make-icon.ps1

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$root = Split-Path $PSScriptRoot -Parent
$assetsDir = Join-Path $root "assets"
$masterPath = Join-Path $assetsDir "quickphrase-logo.png"
$icoPath = Join-Path $assetsDir "quickphrase.ico"

if (-not (Test-Path $masterPath)) {
    throw "Missing master logo PNG: $masterPath"
}

$master = [System.Drawing.Bitmap]::new($masterPath)
$sizes = 16, 24, 32, 48, 64, 128, 256
$pngs = @()

function Resize-IconPng([System.Drawing.Bitmap]$src, [int]$size) {
    $bmp = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)
    $g.DrawImage($src, 0, 0, $size, $size)
    $g.Dispose()

    $ms = [System.IO.MemoryStream]::new()
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bytes = $ms.ToArray()
    $ms.Dispose()
    $bmp.Dispose()
    return $bytes
}

foreach ($size in $sizes) {
    $pngs += ,@($size, (Resize-IconPng $master $size))
}
$master.Dispose()

# ICO container with PNG-compressed 32-bpp entries.
$fs = [System.IO.File]::Create($icoPath)
$bw = [System.IO.BinaryWriter]::new($fs)
$bw.Write([UInt16]0)      # reserved
$bw.Write([UInt16]1)      # type: icon
$bw.Write([UInt16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $size = $pngs[$i][0]
    $data = $pngs[$i][1]
    $dim = if ($size -ge 256) { 0 } else { $size }
    $bw.Write([Byte]$dim)
    $bw.Write([Byte]$dim)
    $bw.Write([Byte]0)
    $bw.Write([Byte]0)
    $bw.Write([UInt16]1)
    $bw.Write([UInt16]32)
    $bw.Write([UInt32]$data.Length)
    $bw.Write([UInt32]$offset)
    $offset += $data.Length
}
foreach ($entry in $pngs) {
    $bw.Write([Byte[]]$entry[1])
}
$bw.Flush()
$bw.Dispose()
$fs.Dispose()

"icon=$icoPath bytes=$((Get-Item $icoPath).Length)"
