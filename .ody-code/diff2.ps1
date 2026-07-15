param([string]$a, [string]$b)
Add-Type -AssemblyName System.Drawing
$ia = [System.Drawing.Bitmap]::FromFile($a)
$ib = [System.Drawing.Bitmap]::FromFile($b)
$w = [Math]::Min($ia.Width, $ib.Width); $h = [Math]::Min($ia.Height, $ib.Height)
$minx = $w; $miny = $h; $maxx = -1; $maxy = -1; $count = 0
# sample every 2px for speed
for ($y = 0; $y -lt $h; $y += 2) {
  for ($x = 0; $x -lt $w; $x += 2) {
    $pa = $ia.GetPixel($x, $y); $pb = $ib.GetPixel($x, $y)
    if ([Math]::Abs($pa.R - $pb.R) -gt 24 -or [Math]::Abs($pa.G - $pb.G) -gt 24 -or [Math]::Abs($pa.B - $pb.B) -gt 24) {
      $count++
      if ($x -lt $minx) { $minx = $x }; if ($x -gt $maxx) { $maxx = $x }
      if ($y -lt $miny) { $miny = $y }; if ($y -gt $maxy) { $maxy = $y }
    }
  }
}
Write-Output "diff px(samples): $count  bbox: ($minx,$miny)-($maxx,$maxy)"
# cluster report: histogram of changed pixels per 40px row band
if ($count -gt 0) {
  $bands = @{}
  for ($y = 0; $y -lt $h; $y += 2) {
    $bc = 0
    for ($x = 0; $x -lt $w; $x += 2) {
      $pa = $ia.GetPixel($x, $y); $pb = $ib.GetPixel($x, $y)
      if ([Math]::Abs($pa.R - $pb.R) -gt 24 -or [Math]::Abs($pa.G - $pb.G) -gt 24 -or [Math]::Abs($pa.B - $pb.B) -gt 24) { $bc++ }
    }
    if ($bc -gt 0) { $band = [int]($y / 20) * 20; $bands[$band] = ($bands[$band] + $bc) }
  }
  $bands.GetEnumerator() | Sort-Object Name | ForEach-Object { Write-Output ("y {0,4}..{1,-4}: {2}" -f $_.Name, ($_.Name + 20), $_.Value) }
}
$ia.Dispose(); $ib.Dispose()
