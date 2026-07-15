param([string]$src, [string]$xs, [string]$ys)
Add-Type -AssemblyName System.Drawing
$bmp = [System.Drawing.Bitmap]::FromFile($src)
foreach ($y in ($ys -split ',')) { foreach ($x in ($xs -split ',')) { $p = $bmp.GetPixel([int]$x, [int]$y); Write-Output ("({0},{1}) = {2},{3},{4}" -f $x, $y, $p.R, $p.G, $p.B) } }
$bmp.Dispose()
