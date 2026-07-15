Add-Type -AssemblyName System.Drawing
$img = New-Object System.Drawing.Bitmap('C:\Users\Administrator\Pictures\000020.png')
$w = $img.Width; $h = $img.Height

# cyan pixels by y (exact-ish color), print row histogram
$rows = @{}
for ($y = 1300; $y -lt $h; $y++) {
  for ($x = 0; $x -lt $w; $x++) {
    $p = $img.GetPixel($x, $y)
    if ([Math]::Abs($p.R - 58) -le 20 -and [Math]::Abs($p.G - 150) -le 20 -and [Math]::Abs($p.B - 221) -le 20) {
      if (-not $rows.ContainsKey($y)) { $rows[$y] = New-Object System.Collections.ArrayList }
      [void]$rows[$y].Add($x)
    }
  }
}
Write-Output "cyan rows (y: count, minX, maxX):"
foreach ($y in ($rows.Keys | Sort-Object)) {
  $xs = $rows[$y]
  Write-Output ("y={0} n={1} minX={2} maxX={3}" -f $y, $xs.Count, ($xs | Measure-Object -Minimum).Minimum, ($xs | Measure-Object -Maximum).Maximum)
}

# vertical profile at x=2508..2512
Write-Output "`ncolumn profile x=2508..2512 (y: colors):"
for ($y = 1330; $y -lt 1440; $y++) {
  $vals = @()
  for ($x = 2508; $x -le 2512; $x++) {
    $p = $img.GetPixel($x, $y)
    $vals += "($($p.R),$($p.G),$($p.B))"
  }
  Write-Output ("y={0}: {1}" -f $y, ($vals -join ' '))
}
