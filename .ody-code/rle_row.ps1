Add-Type -AssemblyName System.Drawing
$img = New-Object System.Drawing.Bitmap('C:\Users\Administrator\Pictures\000020.png')
$w = $img.Width
foreach ($y in @(1378, 1358)) {
  Write-Output "=== row y=$y ==="
  $runs = @()
  $curCls = $null; $start = 0
  for ($x = 0; $x -lt $w; $x++) {
    $p = $img.GetPixel($x, $y)
    $cls = 'other'
    if ($p.R -eq 12 -and $p.G -eq 12 -and $p.B -eq 12) { $cls = 'bg12' }
    elseif ($p.R -eq 41 -and $p.G -eq 41 -and $p.B -eq 41) { $cls = 'bg41' }
    elseif ([Math]::Abs($p.R-58) -le 20 -and [Math]::Abs($p.G-150) -le 20 -and [Math]::Abs($p.B-221) -le 20) { $cls = 'cyan' }
    elseif ($p.R -gt 225 -and $p.G -gt 225 -and $p.B -gt 225) { $cls = 'white' }
    elseif ($p.R -eq 237 -and $p.G -eq 28 -and $p.B -eq 36) { $cls = 'red' }
    elseif ($p.R -lt 30 -and $p.G -lt 30 -and $p.B -lt 30) { $cls = 'dark' }
    elseif ([Math]::Abs($p.R - $p.G) -lt 25 -and [Math]::Abs($p.G - $p.B) -lt 25) { $cls = 'graytext' }
    if ($cls -ne $curCls) {
      if ($null -ne $curCls) { $runs += "$curCls[$start..$($x-1)]" }
      $curCls = $cls; $start = $x
    }
  }
  $runs += "$curCls[$start..$($w-1)]"
  # merge tiny noise runs display: print all runs but compact dark/bg
  Write-Output ($runs -join ' ')
}
