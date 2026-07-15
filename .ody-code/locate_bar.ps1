Add-Type -AssemblyName System.Drawing
$img = New-Object System.Drawing.Bitmap('C:\Users\Administrator\Pictures\000020.png')
$w = $img.Width; $h = $img.Height

# find exact footer-cyan pixels (58,150,221) with small tolerance
$cyanX = New-Object System.Collections.ArrayList
$cyanY = New-Object System.Collections.ArrayList
for ($y = 0; $y -lt $h; $y++) {
  for ($x = 0; $x -lt $w; $x++) {
    $p = $img.GetPixel($x, $y)
    if ([Math]::Abs($p.R - 58) -le 12 -and [Math]::Abs($p.G - 150) -le 12 -and [Math]::Abs($p.B - 221) -le 12) {
      [void]$cyanX.Add($x); [void]$cyanY.Add($y)
    }
  }
}
Write-Output "cyan count: $($cyanX.Count)"
$minX = ($cyanX | Measure-Object -Minimum).Minimum
$maxX = ($cyanX | Measure-Object -Maximum).Maximum
$minY = ($cyanY | Measure-Object -Minimum).Minimum
$maxY = ($cyanY | Measure-Object -Maximum).Maximum
Write-Output "cyan bbox: x=$minX..$maxX y=$minY..$maxY"

# In that y band, scan right of maxX for near-white (243,243,243) bar pixels
$barX = New-Object System.Collections.ArrayList
$barY = New-Object System.Collections.ArrayList
for ($y = $minY - 3; $y -le $maxY + 3; $y++) {
  for ($x = $maxX + 1; $x -lt [Math]::Min($maxX + 80, $w); $x++) {
    $p = $img.GetPixel($x, $y)
    if ($p.R -gt 225 -and $p.G -gt 225 -and $p.B -gt 225 -and [Math]::Abs($p.R - $p.G) -lt 12 -and [Math]::Abs($p.G - $p.B) -lt 12) {
      [void]$barX.Add($x); [void]$barY.Add($y)
    }
  }
}
Write-Output "bar count: $($barX.Count)"
if ($barX.Count -gt 0) {
  Write-Output "bar bbox: x=$(($barX | Measure-Object -Minimum).Minimum)..$(($barX | Measure-Object -Maximum).Maximum) y=$(($barY | Measure-Object -Minimum).Minimum)..$(($barY | Measure-Object -Maximum).Maximum)"
  $barX | Group-Object | Sort-Object {[int]$_.Name} | ForEach-Object { Write-Output "  x=$($_.Name) n=$($_.Count)" }
}
