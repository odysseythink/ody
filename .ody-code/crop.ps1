param([string]$src, [string]$dst, [int]$x, [int]$y, [int]$w, [int]$h, [int]$scale = 1)
Add-Type -AssemblyName System.Drawing
$bmp = [System.Drawing.Bitmap]::FromFile($src)
$crop = New-Object System.Drawing.Bitmap($w, $h)
$g = [System.Drawing.Graphics]::FromImage($crop)
$g.DrawImage($bmp, (New-Object System.Drawing.Rectangle(0, 0, $w, $h)), (New-Object System.Drawing.Rectangle($x, $y, $w, $h)), [System.Drawing.GraphicsUnit]::Pixel)
if ($scale -gt 1) {
  $big = New-Object System.Drawing.Bitmap(($w * $scale), ($h * $scale))
  $g2 = [System.Drawing.Graphics]::FromImage($big)
  $g2.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
  $g2.DrawImage($crop, 0, 0, ($w * $scale), ($h * $scale))
  $big.Save($dst); $g2.Dispose(); $big.Dispose()
} else { $crop.Save($dst) }
$g.Dispose(); $crop.Dispose(); $bmp.Dispose()
Write-Output "saved $dst"
