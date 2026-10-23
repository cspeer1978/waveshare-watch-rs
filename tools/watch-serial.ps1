# Watch serial reader for the Waveshare watch.
# The port is set by $ComPort before running (default COM4), e.g. on the home laptop:
#   $ComPort = 'COM32'; Invoke-Expression (Get-Content "$HOME\Downloads\watch-serial.ps1" -Raw)
# Ctrl+C to stop.
#
# * Opens COM4 WITHOUT toggling DTR/RTS, so the watch keeps running
#   (the esptool web console toggles them, which resets the watch into download mode).
# * Sends "EXPORT" to the watch right away: the watch replies with its whole
#   battery log, which is saved to watch-battery.csv on your Desktop.
#   No need to touch the watch. (The Battery app's USB button does the same.)
# * Everything else the watch prints is shown too (boot messages, Wi-Fi, etc.).
#   Type nothing; to ask again, press Ctrl+C and run it again.

if (-not $ComPort) { $ComPort = 'COM4' }
$port = New-Object System.IO.Ports.SerialPort $ComPort, 115200, 'None', 8, 'One'
$port.DtrEnable = $false
$port.RtsEnable = $false
$port.ReadTimeout = 500
$port.NewLine = "`n"
try {
    $port.Open()
} catch {
    Write-Host "Can't open ${ComPort}: $($_.Exception.InnerException.Message)" -ForegroundColor Red
    Write-Host "Another program has the port. Close the esptool web page / any serial monitor, unplug and replug the watch, then try again." -ForegroundColor Yellow
    return
}
$csv = Join-Path ([Environment]::GetFolderPath('Desktop')) 'watch-battery.csv'
$inCsv = $false
Write-Host "Connected to $ComPort. Asking the watch for its battery log..." -ForegroundColor Cyan
Start-Sleep -Milliseconds 300
$port.WriteLine("EXPORT")
try {
    while ($true) {
        try { $line = $port.ReadLine().TrimEnd("`r") } catch { continue }   # 0.5 s timeout, keep waiting
        Write-Host $line
        if ($line -like '--- BATTERY LOG CSV ---*') { $inCsv = $true; Set-Content -Path $csv -Value $null; continue }
        if ($line -like '--- END*') { $inCsv = $false; Write-Host "Saved to $csv" -ForegroundColor Green; continue }
        if ($inCsv) { Add-Content -Path $csv -Value $line }
    }
} finally {
    $port.Close()
}
