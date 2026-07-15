param([string]$src, [int]$x, [int]$y0, [int]$y1, [int]$r, [int]$g, [int]$b)
Add-Type -AssemblyName System.Drawing
$bmp = [System.Drawing.Bitmap]::FromFile($src)
$runs = @(); $start = -1; $prev = -2
for ($y = $y0; $y -lt $y1; $y++) {
  $p = $bmp.GetPixel($x, $y)
  $m = [math]::Abs($p.R - $r) -le 2 -and [math]::Abs($p.G - $g) -le 2 -and [math]::Abs($p.B - $b) -le 2
  if ($m) { if ($y -ne $prev + 1) { if ($start -ge 0) { $runs += "$start-$prev" }; $start = $y }; $prev = $y }
}
if ($start -ge 0) { $runs += "$start-$prev" }
Write-Output ($runs -join '  ')
$bmp.Dispose()
