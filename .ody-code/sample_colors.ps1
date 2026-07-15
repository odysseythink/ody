Add-Type -AssemblyName System.Drawing
# crop_br_wide is 760x490 showing 【 Design 】 and the bar at right side
$img = New-Object System.Drawing.Bitmap('E:\ody-rs\.ody-code\crop_br_wide.png')
Write-Output "crop size: $($img.Width)x$($img.Height)"
# sample a horizontal strip through the text row (relative y ~425) and print colors of non-background pixels
for ($y = 410; $y -le 440; $y += 6) {
  $line = "y=${y}: "
  for ($x = 600; $x -lt 760; $x += 2) {
    $p = $img.GetPixel($x, $y)
    if ($p.R -gt 60 -or $p.G -gt 60 -or $p.B -gt 60) {
      $line += "x=${x}($($p.R),$($p.G),$($p.B)) "
    }
  }
  Write-Output $line
}
