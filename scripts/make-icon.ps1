# Generates the placeholder app icon: assets/icon.ico (16..256 px, PNG-compressed entries)
# and assets/icon-256.png (window icon). Replace both files with your own artwork any time.
param([string]$OutDir = "$PSScriptRoot\..\assets")
Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Force $OutDir | Out-Null

function New-IconBitmap([int]$s) {
    $bmp = New-Object System.Drawing.Bitmap $s, $s, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.Clear([System.Drawing.Color]::Transparent)
    $r = [Math]::Max(2, [int]($s * 0.22))
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $m = [Math]::Max(0, [int]($s * 0.03)); $w = $s - 2 * $m
    $path.AddArc($m, $m, $r, $r, 180, 90)
    $path.AddArc($m + $w - $r, $m, $r, $r, 270, 90)
    $path.AddArc($m + $w - $r, $m + $w - $r, $r, $r, 0, 90)
    $path.AddArc($m, $m + $w - $r, $r, $r, 90, 90)
    $path.CloseFigure()
    $rect = New-Object System.Drawing.Rectangle 0, 0, $s, $s
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush $rect, ([System.Drawing.Color]::FromArgb(255, 40, 110, 230)), ([System.Drawing.Color]::FromArgb(255, 140, 60, 220)), 45.0
    $g.FillPath($brush, $path)
    # "Up" arrow made of stacked chevrons = enhance / upscale.
    $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::White), ([Math]::Max(1.5, $s * 0.09))
    $pen.StartCap = 'Round'; $pen.EndCap = 'Round'; $pen.LineJoin = 'Round'
    foreach ($k in 0, 1) {
        $y = $s * (0.42 + 0.22 * $k)
        $pts = @(
            (New-Object System.Drawing.PointF ($s * 0.28), ($y + $s * 0.12)),
            (New-Object System.Drawing.PointF ($s * 0.50), ($y - $s * 0.10)),
            (New-Object System.Drawing.PointF ($s * 0.72), ($y + $s * 0.12))
        )
        $g.DrawLines($pen, [System.Drawing.PointF[]]$pts)
    }
    # Small sparkle (top right).
    $sp = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(235, 255, 230, 120))
    $c = $s * 0.76; $cy = $s * 0.22; $a = $s * 0.10; $b = $s * 0.025
    $star = @(
        (New-Object System.Drawing.PointF $c, ($cy - $a)), (New-Object System.Drawing.PointF ($c + $b), ($cy - $b)),
        (New-Object System.Drawing.PointF ($c + $a), $cy), (New-Object System.Drawing.PointF ($c + $b), ($cy + $b)),
        (New-Object System.Drawing.PointF $c, ($cy + $a)), (New-Object System.Drawing.PointF ($c - $b), ($cy + $b)),
        (New-Object System.Drawing.PointF ($c - $a), $cy), (New-Object System.Drawing.PointF ($c - $b), ($cy - $b))
    )
    if ($s -ge 24) { $g.FillPolygon($sp, [System.Drawing.PointF[]]$star) }
    $g.Dispose()
    return $bmp
}

$sizes = 16, 24, 32, 48, 64, 128, 256
$pngs = @()
foreach ($s in $sizes) {
    $bmp = New-IconBitmap $s
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $pngs += , $ms.ToArray()
    if ($s -eq 256) { $bmp.Save("$OutDir\icon-256.png", [System.Drawing.Imaging.ImageFormat]::Png) }
    $bmp.Dispose()
}

$out = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter $out
$bw.Write([UInt16]0); $bw.Write([UInt16]1); $bw.Write([UInt16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i]; $len = $pngs[$i].Length
    $dim = if ($s -ge 256) { 0 } else { $s }
    $bw.Write([byte]$dim); $bw.Write([byte]$dim); $bw.Write([byte]0); $bw.Write([byte]0)
    $bw.Write([UInt16]1); $bw.Write([UInt16]32); $bw.Write([UInt32]$len); $bw.Write([UInt32]$offset)
    $offset += $len
}
foreach ($p in $pngs) { $bw.Write($p) }
$bw.Flush()
[System.IO.File]::WriteAllBytes("$OutDir\icon.ico", $out.ToArray())
Write-Output "wrote $OutDir\icon.ico ($($out.Length) bytes) and icon-256.png"
