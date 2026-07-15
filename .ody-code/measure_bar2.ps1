Add-Type -AssemblyName System.Drawing
$img = New-Object System.Drawing.Bitmap('C:\Users\Administrator\Pictures\000020.png')
$w = $img.Width; $h = $img.Height

# Find cyan-ish pixels (the 【 Design 】 footer text) in bottom 250 rows
$cyanPts = @()
for ($y = $h - 250; $y -lt $h; $y++) {
  for ($x = 0; $x -lt $w; $x++) {
    $p = $img.GetPixel($x, $y)
    if ($p.B -gt 140 -and $p.G -gt 120 -and $p.R -lt 140 -and ($p.B - $p.R) -gt 40) {
      $cyanPts += ,@($x, $y)
    }
  }
}
Write-Output "cyan pixel count: $($cyanPts.Count)"
if ($cyanPts.Count -gt 0) {
  $cxs = $cyanPts | ForEach-Object { $_[0] }
  $cys = $cyanPts | ForEach-Object { $_[1] }
  $minX = ($cxs | Measure-Object -Minimum).Minimum
  $maxX = ($cxs | Measure-Object -Maximum).Maximum
  $minY = ($cys | Measure-Object -Minimum).Minimum
  $maxY = ($cys | Measure-Object -Maximum).Maximum
  Write-Output "cyan x range: $minX - $maxX"
  Write-Output "cyan y range: $minY - $maxY"
  # rightmost cyan column = right edge of 】
  # Now find bright white bar pixels in same y band, to the right of maxX
  $barPts = @()
  for ($y = $minY - 4; $y -le $maxY + 4; $y++) {
    for ($x = $maxX + 1; $x -lt [Math]::Min($maxX + 60, $w); $x++) {
      $p = $img.GetPixel($x, $y)
      if ($p.R -gt 180 -and $p.G -gt 180 -and $p.B -gt 180) {
        $barPts += ,@($x, $y)
      }
    }
  }
  Write-Output "bar pixel count right of cyan: $($barPts.Count)"
  if ($barPts.Count -gt 0) {
    $bxs = $barPts | ForEach-Object { $_[0] }
    $bys = $barPts | ForEach-Object { $_[1] }
    Write-Output "bar x range: $(($bxs | Measure-Object -Minimum).Minimum) - $(($bxs | Measure-Object -Maximum).Maximum)"
    Write-Output "bar y range: $(($bys | Measure-Object -Minimum).Minimum) - $(($bys | Measure-Object -Maximum).Maximum)"
  }
  # leftmost text anchor: composer row '>' glyph - find leftmost non-bg pixel rows just above cyan band
}
