param([string]$title, [string]$keys, [int]$delayMs = 0)
Add-Type -AssemblyName System.Windows.Forms
Add-Type '[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr h);' -Name U32 -Namespace Win
$p = Get-Process WindowsTerminal -ErrorAction Stop | Where-Object { $_.MainWindowTitle -like "*$title*" } | Select-Object -First 1
if (-not $p) { Write-Output "NO WINDOW matching $title"; exit 1 }
[Win.U32]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
Start-Sleep -Milliseconds 600
if ($delayMs -gt 0) { Start-Sleep -Milliseconds $delayMs }
[System.Windows.Forms.SendKeys]::SendWait($keys)
Write-Output "sent [$keys] to $($p.MainWindowTitle)"
