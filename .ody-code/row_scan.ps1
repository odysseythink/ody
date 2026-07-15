param([string]$src, [int]$y, [int]$x0, [int]$x1)
Add-Type -AssemblyName System.Drawing
$bmp = [System.Drawing.Bitmap]::FromFile($src)
$runs = @(); $start = -1; $prev = -2
for ($x = $x0; $x -lt $x1; $x++) {
  $p = $bmp.GetPixel($x, $y)
  $m = $p.R -gt 60 -or $p.G -gt 60 -or $p.B -gt 60
  if ($m) { if ($x -ne $prev + 1) { if ($start -ge 0) { $runs += "$start-$prev" }; $start = $x }; $prev = $x }
}
if ($start -ge 0) { $runs += "$start-$prev" }
Write-Output ($runs -join '  ')
$bmp.Dispose()
