[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$projectDir = Split-Path -Parent $PSScriptRoot
$iconDir = Join-Path $projectDir 'packaging\icons'
$pngDir = Join-Path $iconDir 'png'
New-Item -ItemType Directory -Force -Path $pngDir | Out-Null

function New-ScaledPoint([double]$X, [double]$Y, [double]$Scale) {
    New-Object System.Drawing.PointF ([single]($X * $Scale)), ([single]($Y * $Scale))
}

function New-LitManPng([int]$Size) {
    $scale = $Size / 128.0
    $bitmap = New-Object System.Drawing.Bitmap $Size, $Size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    try {
        $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.Clear([System.Drawing.Color]::Transparent)

        $background = New-Object System.Drawing.Drawing2D.GraphicsPath
        $diameter = [single](48 * $scale)
        $extent = [single]($Size - $diameter)
        $background.AddArc(0, 0, $diameter, $diameter, 180, 90)
        $background.AddArc($extent, 0, $diameter, $diameter, 270, 90)
        $background.AddArc($extent, $extent, $diameter, $diameter, 0, 90)
        $background.AddArc(0, $extent, $diameter, $diameter, 90, 90)
        $background.CloseFigure()
        $blue = New-Object System.Drawing.SolidBrush ([System.Drawing.ColorTranslator]::FromHtml('#315b7d'))
        $graphics.FillPath($blue, $background)

        $left = New-Object System.Drawing.Drawing2D.GraphicsPath
        $left.StartFigure()
        $left.AddBezier((New-ScaledPoint 25 28 $scale), (New-ScaledPoint 42 22 $scale), (New-ScaledPoint 56 25 $scale), (New-ScaledPoint 64 33 $scale))
        $left.AddLine((New-ScaledPoint 64 33 $scale), (New-ScaledPoint 64 104 $scale))
        $left.AddBezier((New-ScaledPoint 64 104 $scale), (New-ScaledPoint 55 96 $scale), (New-ScaledPoint 42 93 $scale), (New-ScaledPoint 25 98 $scale))
        $left.CloseFigure()
        $cream = New-Object System.Drawing.SolidBrush ([System.Drawing.ColorTranslator]::FromHtml('#f4f1e8'))
        $graphics.FillPath($cream, $left)

        $right = New-Object System.Drawing.Drawing2D.GraphicsPath
        $right.StartFigure()
        $right.AddBezier((New-ScaledPoint 103 28 $scale), (New-ScaledPoint 86 22 $scale), (New-ScaledPoint 72 25 $scale), (New-ScaledPoint 64 33 $scale))
        $right.AddLine((New-ScaledPoint 64 33 $scale), (New-ScaledPoint 64 104 $scale))
        $right.AddBezier((New-ScaledPoint 64 104 $scale), (New-ScaledPoint 73 96 $scale), (New-ScaledPoint 86 93 $scale), (New-ScaledPoint 103 98 $scale))
        $right.CloseFigure()
        $white = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::White)
        $graphics.FillPath($white, $right)

        $spinePen = New-Object System.Drawing.Pen ([System.Drawing.ColorTranslator]::FromHtml('#d5cfc2')), ([single]([Math]::Max(1, 3 * $scale)))
        $graphics.DrawLine($spinePen, (New-ScaledPoint 64 33 $scale), (New-ScaledPoint 64 104 $scale))
        $linePen = New-Object System.Drawing.Pen ([System.Drawing.ColorTranslator]::FromHtml('#315b7d')), ([single]([Math]::Max(1, 5 * $scale)))
        $linePen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
        $linePen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
        foreach ($line in @(@(35,45,55,45), @(35,57,55,57), @(73,45,93,45), @(73,57,93,57))) {
            $graphics.DrawLine($linePen, (New-ScaledPoint $line[0] $line[1] $scale), (New-ScaledPoint $line[2] $line[3] $scale))
        }
    }
    finally {
        $graphics.Dispose()
    }
    $path = Join-Path $pngDir "litman-$Size.png"
    $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
    return $path
}

$sizes = @(16, 32, 48, 64, 128, 256, 512, 1024)
$pngs = @{}
foreach ($size in $sizes) {
    $pngs[$size] = New-LitManPng $size
}

$icoSizes = @(16, 32, 48, 64, 128, 256)
$icoImages = foreach ($size in $icoSizes) { ,([System.IO.File]::ReadAllBytes($pngs[$size])) }
$icoStream = New-Object System.IO.MemoryStream
$icoWriter = New-Object System.IO.BinaryWriter $icoStream
$icoWriter.Write([uint16]0)
$icoWriter.Write([uint16]1)
$icoWriter.Write([uint16]$icoSizes.Count)
$offset = 6 + (16 * $icoSizes.Count)
for ($index = 0; $index -lt $icoSizes.Count; $index++) {
    $size = $icoSizes[$index]
    $image = $icoImages[$index]
    $icoWriter.Write([byte]$(if ($size -eq 256) { 0 } else { $size }))
    $icoWriter.Write([byte]$(if ($size -eq 256) { 0 } else { $size }))
    $icoWriter.Write([byte]0)
    $icoWriter.Write([byte]0)
    $icoWriter.Write([uint16]1)
    $icoWriter.Write([uint16]32)
    $icoWriter.Write([uint32]$image.Length)
    $icoWriter.Write([uint32]$offset)
    $offset += $image.Length
}
foreach ($image in $icoImages) { $icoWriter.Write($image) }
$icoWriter.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $iconDir 'litman.ico'), $icoStream.ToArray())
$icoWriter.Dispose()
$icoStream.Dispose()

function Add-BigEndianUInt32([System.Collections.Generic.List[byte]]$Bytes, [uint32]$Value) {
    $Bytes.Add([byte](($Value -shr 24) -band 255))
    $Bytes.Add([byte](($Value -shr 16) -band 255))
    $Bytes.Add([byte](($Value -shr 8) -band 255))
    $Bytes.Add([byte]($Value -band 255))
}

$icnsEntries = @(@('ic07', 128), @('ic08', 256), @('ic09', 512), @('ic10', 1024))
$icnsLength = 8
foreach ($entry in $icnsEntries) { $icnsLength += 8 + ([System.IO.FileInfo]$pngs[$entry[1]]).Length }
$icns = New-Object 'System.Collections.Generic.List[byte]'
$icns.AddRange([System.Text.Encoding]::ASCII.GetBytes('icns'))
Add-BigEndianUInt32 $icns ([uint32]$icnsLength)
foreach ($entry in $icnsEntries) {
    $image = [System.IO.File]::ReadAllBytes($pngs[$entry[1]])
    $icns.AddRange([System.Text.Encoding]::ASCII.GetBytes([string]$entry[0]))
    Add-BigEndianUInt32 $icns ([uint32](8 + $image.Length))
    $icns.AddRange($image)
}
[System.IO.File]::WriteAllBytes((Join-Path $iconDir 'litman.icns'), $icns.ToArray())

Write-Host "Generated LitMan PNG, ICO, and ICNS assets from packaging/icons/litman.svg."
