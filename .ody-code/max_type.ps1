param([string]$title, [string]$keys)
Add-Type -AssemblyName System.Windows.Forms
Add-Type '[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool SetForegroundWindow(System.IntPtr h); [System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool ShowWindow(System.IntPtr h, int c);' -Name U32 -Namespace Win
$p = Get-Process WindowsTerminal -ErrorAction Stop | Where-Object { $_.MainWindowTitle -like "*$title*" } | Select-Object -First 1
if (-not $p) { Write-Output "NO WINDOW matching $title"; exit 1 }
[Win.U32]::ShowWindow($p.MainWindowHandle, 3) | Out-Null  # SW_MAXIMIZE
[Win.U32]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
Start-Sleep -Milliseconds 700
[System.Windows.Forms.SendKeys]::SendWait($keys)
Write-Output "maximized + sent keys to $($p.MainWindowTitle)"
