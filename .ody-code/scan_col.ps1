param([string]$src, [int]$x, [int]$y0, [int]$y1, [int]$r, [int]$g, [int]$b)
Add-Type -AssemblyName System.Drawing
$bmp = [System.Drawing.Bitmap]::FromFile($src)
$found = @()
for ($y = $y0; $y -lt $y1; $y++) {
  $p = $bmp.GetPixel($x, $y)
  if ([math]::Abs($p.R - $r) -le 2 -and [math]::Abs($p.G - $g) -le 2 -and [math]::Abs($p.B - $b) -le 2) { $found += $y }
}
Write-Output ("count={0}" -f $found.Count)
if ($found.Count -gt 0) { Write-Output ("first={0} last={1}" -f $found[0], $found[-1]) }
$bmp.Dispose()
