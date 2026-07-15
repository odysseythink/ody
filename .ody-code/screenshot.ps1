Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
$b = [System.Windows.Forms.SystemInformation]::VirtualScreen
Write-Output "virtual screen: $($b.Width)x$($b.Height) at $($b.Left),$($b.Top)"
$bmp = New-Object System.Drawing.Bitmap($b.Width, $b.Height)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)
$bmp.Save('E:\ody-rs\.ody-code\screen_now.png')
Write-Output "saved"
# list visible windows with titles
Get-Process | Where-Object {$_.MainWindowTitle -ne ""} | Select-Object ProcessName, MainWindowTitle | Format-Table -AutoSize | Out-String -Width 300
