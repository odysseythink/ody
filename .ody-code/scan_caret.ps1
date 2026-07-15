param([string]$src, [int]$y0, [int]$y1, [int]$x0, [int]$x1)
Add-Type -AssemblyName System.Drawing
$bmp = [System.Drawing.Bitmap]::FromFile($src)
for ($y = $y0; $y -lt $y1; $y++) {
  $hits = @()
  for ($x = $x0; $x -lt $x1; $x++) {
    $p = $bmp.GetPixel($x, $y)
    if ($p.R -gt 200 -and $p.G -gt 200 -and $p.B -gt 200) { $hits += $x }
  }
  if ($hits.Count -gt 0) { Write-Output ("y={0} xs={1}" -f $y, ($hits -join ',')) }
}
$bmp.Dispose()
