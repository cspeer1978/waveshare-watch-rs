#!/bin/sh
# Re-render every screenshot in docs/screenshots (Linux; on macOS pass --target <your triple>).
# Uses placeholder Wi-Fi names and a dummy password, never your real ones.
set -e
cd "$(dirname "$0")"
export HA_TOKEN=preview-only WIFI_SSID=HomeWiFi WIFI_PASS=example-password WIFI2_SSID=PhoneHotspot
cargo run --release "$@" -- gallery
cargo run --release "$@" -- battery > /dev/null
cp out/real_mid_test.png ../../docs/screenshots/15b_battery.png
cp out/real_on_usb.png ../../docs/screenshots/15c_battery_charging.png
