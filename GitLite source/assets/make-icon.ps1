# Rebuilds GitLite.ico from the original mark. Run on Windows.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

function Get-Color([string] $Hex) {
    [System.Drawing.ColorTranslator]::FromHtml($Hex)
}

function New-Frame([int] $Size) {
    $canvas = [System.Drawing.Bitmap]::new($Size * 4, $Size * 4)
    $frame = [System.Drawing.Bitmap]::new($Size, $Size)
    $art = [System.Drawing.Graphics]::FromImage($canvas)
    $renderer = [System.Drawing.Graphics]::FromImage($frame)
    $tile = [System.Drawing.Drawing2D.GraphicsPath]::new()
    $bounds = [System.Drawing.RectangleF]::new(8, 8, 240, 240)
    $background = [System.Drawing.Drawing2D.LinearGradientBrush]::new(
        $bounds, (Get-Color '#20374E'), (Get-Color '#111C2C'), [single] 55
    )
    $rim = [System.Drawing.Pen]::new((Get-Color '#395268'), [single] 2)
    $branch = [System.Drawing.Pen]::new((Get-Color '#55DCA0'), [single] 15)
    $ring = [System.Drawing.Pen]::new((Get-Color '#55DCA0'), [single] 13.5)
    $stream = [System.IO.MemoryStream]::new()

    try {
        $art.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $art.ScaleTransform($canvas.Width / 256, $canvas.Height / 256)

        $tile.AddArc(8, 8, 94, 94, 180, 90)
        $tile.AddArc(154, 8, 94, 94, 270, 90)
        $tile.AddArc(154, 154, 94, 94, 0, 90)
        $tile.AddArc(8, 154, 94, 94, 90, 90)
        $tile.CloseFigure()
        $art.FillPath($background, $tile)
        if ($Size -ge 32) {
            $art.DrawPath($rim, $tile)
        }

        $art.DrawLine($branch, 89, 68, 89, 187)
        $art.DrawBezier($branch, 89, 151, 142, 151, 173, 117, 173, 68)
        $commits = @(
            [System.Drawing.Point]::new(89, 68)
            [System.Drawing.Point]::new(173, 68)
            [System.Drawing.Point]::new(89, 187)
        )
        foreach ($commit in $commits) {
            $node = [System.Drawing.Rectangle]::new($commit.X - 17, $commit.Y - 17, 34, 34)
            $art.FillEllipse($background, $node)
            $art.DrawEllipse($ring, $node)
        }

        $renderer.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $renderer.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $renderer.DrawImage($canvas, 0, 0, $Size, $Size)
        $frame.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)

        [pscustomobject] @{ Size = $Size; Bytes = $stream.ToArray() }
    }
    finally {
        foreach ($item in @($ring, $branch, $rim, $background, $tile, $art, $renderer, $canvas, $frame, $stream)) {
            $item.Dispose()
        }
    }
}

$frames = @(16, 20, 24, 32, 40, 48, 64, 96, 128, 256 | ForEach-Object { New-Frame $_ })
$offset = 6 + 16 * $frames.Count
$file = [System.IO.File]::Create((Join-Path $PSScriptRoot 'GitLite.ico'))
$writer = [System.IO.BinaryWriter]::new($file)

try {
    $writer.Write([uint16] 0)
    $writer.Write([uint16] 1)
    $writer.Write([uint16] $frames.Count)
    foreach ($frame in $frames) {
        $dimension = [byte] ($frame.Size % 256)
        $writer.Write($dimension)
        $writer.Write($dimension)
        $writer.Write([uint16] 0)
        $writer.Write([uint16] 1)
        $writer.Write([uint16] 32)
        $writer.Write([uint32] $frame.Bytes.Length)
        $writer.Write([uint32] $offset)
        $offset += $frame.Bytes.Length
    }
    foreach ($frame in $frames) {
        $writer.Write([byte[]] $frame.Bytes)
    }
}
finally {
    $writer.Dispose()
}
