# Generates assets/quickphrase.ico from code (no external assets).
# Usage: powershell -ExecutionPolicy Bypass -File scripts\make-icon.ps1

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$root = Split-Path $PSScriptRoot -Parent
$outDir = Join-Path $root "assets"
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$icoPath = Join-Path $outDir "quickphrase.ico"

$sizes = 16, 24, 32, 48, 64, 128, 256
$pngs = @()

function New-IconPng([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.Clear([System.Drawing.Color]::Transparent)

    # Rounded square in the product primary colour.
    $pad = [Math]::Max(1, [int]($size * 0.06))
    $rect = New-Object System.Drawing.Rectangle($pad, $pad, ($size - 2 * $pad), ($size - 2 * $pad))
    $radius = [Math]::Max(2, [int]($size * 0.22))
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $radius * 2
    $path.AddArc($rect.X, $rect.Y, $d, $d, 180, 90)
    $path.AddArc($rect.Right - $d, $rect.Y, $d, $d, 270, 90)
    $path.AddArc($rect.Right - $d, $rect.Bottom - $d, $d, $d, 0, 90)
    $path.AddArc($rect.X, $rect.Bottom - $d, $d, $d, 90, 90)
    $path.CloseFigure()
    $brush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 0x76, 0x55, 0xE9))
    $g.FillPath($brush, $path)

    # White speech bubble with three text lines.
    $bubbleX = $size * 0.20
    $bubbleY = $size * 0.24
    $bubbleW = $size * 0.60
    $bubbleH = $size * 0.42
    $bubble = New-Object System.Drawing.RectangleF($bubbleX, $bubbleY, $bubbleW, $bubbleH)
    $bubbleRadius = [Math]::Max(1, [int]($size * 0.08))
    $bd = $bubbleRadius * 2
    $bubblePath = New-Object System.Drawing.Drawing2D.GraphicsPath
    $bubblePath.AddArc($bubble.X, $bubble.Y, $bd, $bd, 180, 90)
    $bubblePath.AddArc($bubble.Right - $bd, $bubble.Y, $bd, $bd, 270, 90)
    $bubblePath.AddArc($bubble.Right - $bd, $bubble.Bottom - $bd, $bd, $bd, 0, 90)
    $bubblePath.AddArc($bubble.X, $bubble.Bottom - $bd, $bd, $bd, 90, 90)
    $bubblePath.CloseFigure()
    $white = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::White)
    $g.FillPath($white, $bubblePath)

    # Tail of the bubble.
    $tail = New-Object System.Drawing.Drawing2D.GraphicsPath
    $tail.AddPolygon(@(
        (New-Object System.Drawing.PointF(($size * 0.30), ($size * 0.64))),
        (New-Object System.Drawing.PointF(($size * 0.46), ($size * 0.64))),
        (New-Object System.Drawing.PointF(($size * 0.30), ($size * 0.78)))
    ))
    $g.FillPath($white, $tail)

    # Lines inside the bubble (purple).
    $lineBrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(255, 0x76, 0x55, 0xE9))
    $lineH = [Math]::Max(1, [int]($size * 0.055))
    $lineX = $size * 0.28
    $lineW = $size * 0.44
    foreach ($offset in 0.32, 0.43, 0.54) {
        $y = $size * $offset
        $g.FillRectangle($lineBrush, [single]$lineX, [single]$y, [single]$lineW, [single]$lineH)
    }

    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bytes = $ms.ToArray()
    $ms.Dispose()
    $g.Dispose()
    $bmp.Dispose()
    return $bytes
}

foreach ($size in $sizes) {
    $pngs += ,@($size, (New-IconPng $size))
}

# ICO container with PNG-compressed entries.
$fs = [System.IO.File]::Create($icoPath)
$bw = New-Object System.IO.BinaryWriter($fs)
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
