Add-Type -AssemblyName System.Drawing
$img = New-Object System.Drawing.Bitmap('C:\Users\Administrator\Pictures\000020.png')
$w = $img.Width; $h = $img.Height
Write-Output "size: ${w}x${h}"

# Scan bottom 200 rows for bright (near-white) pixels to find the bar
$results = @()
for ($y = $h - 200; $y -lt $h; $y++) {
  for ($x = 0; $x -lt $w; $x++) {
    $p = $img.GetPixel($x, $y)
    if ($p.R -gt 200 -and $p.G -gt 200 -and $p.B -gt 200) {
      $results += "$x,$y"
    }
  }
}
Write-Output "bright pixel count: $($results.Count)"
if ($results.Count -gt 0) {
  $xs = $results | ForEach-Object { [int]($_ -split ',')[0] }
  $ys = $results | ForEach-Object { [int]($_ -split ',')[1] }
  Write-Output "x range: $(($xs | Measure-Object -Minimum).Minimum) - $(($xs | Measure-Object -Maximum).Maximum)"
  Write-Output "y range: $(($ys | Measure-Object -Minimum).Minimum) - $(($ys | Measure-Object -Maximum).Maximum)"
  # cluster by x: show histogram of columns with bright pixels
  $xs | Group-Object | Sort-Object {[int]$_.Name} | ForEach-Object { Write-Output "x=$($_.Name) count=$($_.Count)" } | Select-Object -First 40
}
