# Re-render every screenshot in docs/screenshots (Windows).
# Uses placeholder Wi-Fi names and a dummy password, never your real ones.
$env:HA_TOKEN = 'preview-only'
$env:WIFI_SSID = 'HomeWiFi'; $env:WIFI_PASS = 'example-password'; $env:WIFI2_SSID = 'PhoneHotspot'
Push-Location $PSScriptRoot
cargo run --release --target x86_64-pc-windows-msvc -- gallery
cargo run --release --target x86_64-pc-windows-msvc -- battery | Out-Null
Copy-Item out\real_mid_test.png ..\..\docs\screenshots\15b_battery.png
Copy-Item out\real_on_usb.png ..\..\docs\screenshots\15c_battery_charging.png
Pop-Location
