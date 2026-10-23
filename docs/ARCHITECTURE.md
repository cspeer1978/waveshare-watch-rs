# Firmware architecture

This document walks through how the firmware is structured: what runs at boot, how the main loop works, how the screen, sleep and radio are managed, how apps plug in, and the hardware quirks that shaped the design.

- [Big picture](#big-picture)
- [Source tree](#source-tree)
- [Boot sequence](#boot-sequence)
- [The main loop](#the-main-loop)
- [Screen and power states](#screen-and-power-states)
- [Light sleep and waking](#light-sleep-and-waking)
- [Display pipeline](#display-pipeline)
- [Input](#input)
- [Apps](#apps)
- [Networking and Home Assistant](#networking-and-home-assistant)
- [Battery log](#battery-log)
- [Diagnostics and recovery](#diagnostics-and-recovery)
- [Memory](#memory)
- [Hardware quirks worth knowing](#hardware-quirks-worth-knowing)
- [The screenshot renderer](#the-screenshot-renderer)

## Big picture

```
            ┌──────────────────────────── main() : one Embassy task ─────────────────────────────┐
            │                                                                                     │
 GPIO38 ───►│  wait: select( tick timer , touch falling edge , BOOT falling edge )                │
 (touch)    │        └─ or light sleep (screen off, on battery): 5-min timer + touch/BOOT wake    │
 GPIO0  ───►│                                                                                     │
 (BOOT)     │  sensors → clock → battery/log → serial cmds → touch → screen state → Wi-Fi → BLE   │
            │  → timers/alarm → AOD path → app state machine → render into PSRAM framebuffer      │
            │                                                      │                              │
            └──────────────────────────────────────────────────────┼──────────────────────────────┘
                                                                   ▼
          I2C 400 kHz: AXP2101 · FT3168 · PCF85063A · QMI8658 · ES8311/ES7210     QSPI DMA → CO5300 AMOLED
                                                                   
            net_task (Embassy) ── embassy-net stack ── esp-radio Wi-Fi   (only while Wi-Fi is on)
```

Everything that matters happens in **one async task** (`main`). It never busy-waits: every pass ends by waiting on the earliest of a computed timeout, a touch interrupt, or the BOOT button. The only other task is Embassy's network runner, which does work only while the radio is on.

Drawing code never touches hardware directly. It draws into an `embedded-graphics` `DrawTarget`, normally the PSRAM framebuffer. That separation is what lets the same UI code run on a PC in [`tools/preview`](../tools/preview).

## Source tree

| Path | Lines | What it is |
|---|---:|---|
| `src/main.rs` | ~2500 | Boot sequence, crash/breadcrumb capture, NTP, the main loop and its state machines |
| `src/board.rs` | 61 | Pin map and I2C addresses |
| **drivers/** | | **Display stack** |
| `qspi_bus.rs` | 145 | QSPI bus on `SpiDmaBus`, command/pixel framing for the CO5300 |
| `co5300.rs` | 290 | Panel driver: init sequence, brightness, on/off/sleep, windowed pixel writes |
| `framebuffer.rs` | 227 | 410×502 RGB565 framebuffer in PSRAM; full or region DMA flush |
| **peripherals/** | | **One module per chip or service** |
| `power.rs` | 166 | AXP2101: battery %, mV, charging state, VBUS, raw register access |
| `touch.rs` | 189 | FT3168 touch, tap and swipe detection |
| `rtc.rs` | 120 | PCF85063A: read/set BCD time |
| `imu.rs` | 152 | QMI8658 accelerometer/gyro, power up/down on demand |
| `audio.rs` | 145 | ES8311 codec init and I2S DMA beeps |
| `sdcard.rs` | 26 | SPI3 microSD (embedded-sdmmc) |
| `wifi.rs` | 75 | Wi-Fi credentials and config |
| `ble.rs` | 83 | BLE advertising via raw HCI |
| `http.rs` | 265 | Tiny HTTP/1.1 client over embassy-net TCP; HA template API |
| `webexport.rs` | 93 | One-shot HTTP server for the battery CSV |
| `battlog.rs` | 259 | Battery log ring buffer in raw flash |
| `cpu_clock.rs` | 51 | Runtime CPU frequency switching (80/160/240 MHz) |
| `power_stats.rs` | 132 | Live current-draw estimator for the Power page |
| **ui/** | | **System screens** |
| `watchface.rs` | 800 | Watch face, AOD face, touch zones, dirty-region tracking |
| `bigclock.rs` | 36 | Big Clock page |
| `pages.rs` | 173 | Swipe pages (Sensors, System) |
| `power_page.rs` | 327 | Power monitor page |
| `launcher.rs` | 179 | Scrollable app list with drag, fling and tap |
| `segments.rs` | 131 | 7-segment digit renderer used everywhere |
| `t9_keyboard.rs` | 221 | T9 multi-tap text entry |
| **apps/** | | One file per app, plus `widgets.rs` (shared buttons, titles, text helpers) and `mod.rs` (trait + `AppState`) |

## Boot sequence

`main()` (in order):

1. **Heap**: 192 KB internal-RAM heap plus a PSRAM allocator.
2. **Crash report**: read the persistent RTC-memory crash slot and breadcrumb trail left by the previous run.
3. **I2C bus recovery**: if a device is holding SDA low after a brown-out or reset mid-transfer, clock SCL by hand until it lets go, then create the I2C master with a 50 ms software timeout so a stuck device can never block forever.
4. **Power** (AXP2101), **display** (QSPI 80 MHz DMA, TE sync), **framebuffer** in PSRAM, **touch**, **RTC**, **IMU** (powered down right away), **SD card**, **audio** (codec shut down after the boot beep).
5. **Radio stack** is created but left **off**; BLE is created lazily on first use.
6. **Boot diagnostics**: reset reason, super-watchdog and brown-out flags, power-chip power-on/off source registers, slow-clock calibration. These go to the serial console and into the battery log's boot entry.
7. **RTC watchdog** (10 min) is armed and fed at the top of every loop pass.
8. Enter the main loop.

## The main loop

Each pass runs these stages in order. The numbers are the breadcrumb IDs recorded with `crumb()`, so after a crash you can see which stage was last reached.

| Crumb | Stage | Notes |
|---:|---|---|
| 1 | Loop top | Feed RTC watchdog, check brown-out/glitch flags |
| — | **Tick budget** | Decide the longest allowed wait for the current state (table below) |
| 10–11 | **Light sleep** | Only with the screen off, on battery, nothing running |
| 12–13 | **Wait** | `select3(Timer::after(tick), touch_int.wait_for_falling_edge(), boot.wait_for_falling_edge())` |
| 20 | Sensors | IMU sampled only when an app or the gyro ball needs it |
| 30 | Clock | RTC read at 1 Hz while the screen is on |
| 40–43 | Battery + log | Battery %, mV, charging, USB; drain estimate; battery-log record when due |
| — | Serial commands | On USB, read the USB-Serial-JTAG FIFO for `EXPORT` |
| 50 | Touch | Read FT3168 only in interactive screen states; taps and swipes |
| — | Screen state | Wake on input, dim/AOD/off on idle |
| 60–61 | Wi-Fi state machine | Connect (two networks, in order), NTP, idle shut-off |
| — | BLE, timers, alarm | Countdown timer end and daily alarm also fire from sleep |
| — | AOD path | Redraw only when the minute changes |
| 70 | App state machine | Route input to the current app, render, flush |

### Tick budget

The wait is as long as the current state allows:

| State | Max wait |
|---|---|
| Finger down / dragging | 16 ms (~60 Hz) |
| Steps app counting | ~50 ms (20 Hz accelerometer) |
| Screen fully off | 30 s, or light sleep (see below) |
| AOD | 10 s (catch the minute change) |
| Clock page | 1 s (33 ms with the gyro ball on) |
| Sensors page | 100 ms |
| System / Big Clock page | 2 s |
| Launcher | 16 ms while moving, 100 ms idle |
| Fluid sims (Waves, Ball Pit) | 16 ms |
| Static apps (Sun & Moon, Wi-Fi QR) | 30 s |
| Running countdown timer | never later than its end |
| On USB | at most 1 s (to service serial commands) |

Touch and BOOT always cut the wait short.

## Screen and power states

```
   any touch / BOOT                    idle 8 s            idle 15 s              idle 3 min
 ┌──────────────────► 3 BRIGHT ───────────────► 2 DIM ───────────────► 1 AOD ───────────────► 0 OFF
 │                    interactive            interactive         dim HH:MM, 1 redraw/min   panel asleep
 └────────────────────────────────────────────────────────────────────────────────────────────────┘
```

- AOD only happens on the clock pages; from any app, idle goes straight to off.
- Entering OFF drops the CPU to 80 MHz (unless Wi-Fi is on); waking restores the user's speed before re-initialising the panel.
- With the screen OFF and **no USB cable**, the loop uses light sleep instead of a plain wait. On USB the chip stays awake so flashing and the serial console keep working.

## Light sleep and waking

```rust
let t0 = slow_ticks();                                   // RTC slow-clock counter, fresh value
lp_rtc.sleep_light(&[&timer_src /* 300 s */, &gpio_src /* touch + BOOT low */]);
Delay::new().delay_millis(5);                            // let clocks and pads settle before I2C
let slept = slow_ticks() - t0;
let early = slept + 3 * hz < 300 * hz;                   // woke > 3 s early = touch or BOOT
```

- **Wake sources**: a 5-minute timer (housekeeping: battery check, log entry, alarm check), the touch interrupt (GPIO38) and the BOOT button (GPIO0), both low-level.
- **Telling a touch from the timer**: the chip's wake-cause register reports the BOOT button but *not* the touch panel (GPIO38 isn't an RTC pin). So the firmware measures the sleep on the RTC slow-clock counter, the same counter the wake timer runs on. A timer wake measures exactly the planned ticks; anything clearly shorter was the user, and the screen turns on.
- **Reading that counter correctly**: you must request an update (`RTC_CNTL_TIME_UPDATE`) and give it a few slow-clock periods before reading `TIME_LOW0/HIGH0`. Read immediately and you can get the value from the *previous* update. The firmware's `slow_ticks()` waits; it is also called right before sleeping so the wake alarm is computed from a fresh value.
- The Alarm app shortens the sleep so the watch wakes exactly at alarm time.

## Display pipeline

```
 app/ui code ──embedded-graphics──► Framebuffer (PSRAM, 410×502×2 B ≈ 402 KB)
                                            │  full frame, or a dirty region (time digits, battery, gyro ball)
                                            ▼
                               QspiBus (SpiDmaBus, 80 MHz, 4 data lines)
                                            ▼
                                CO5300 panel (col offset 22, RGB565)
```

- The watch face tracks dirty regions (`FlushRegion`) so a seconds tick flushes only the time digits, not 400 KB.
- The panel's TE (tearing-effect) line is used to avoid tearing during full-frame flushes.
- Brightness is a panel command (0–255): the slider on the watch face, `0x40` when dimmed, `0x18` in AOD.

## Input

- **Touch** (FT3168 on I2C, interrupt on GPIO38): the driver turns raw points into taps and swipes (left/right/up/down). Touch is only polled in interactive states; AOD and OFF rely on the interrupt to wake.
- **BOOT button** (GPIO0): Back in every app; wakes the watch.
- **PWR button**: handled by the AXP2101 (hold 10 s to force power-off).
- **IMU**: accelerometer for tilt games, the level, dice shaking and steps; powered on only while one of those is open.

## Apps

Every app is a plain struct in `src/apps/`:

```rust
pub trait App {
    fn name(&self) -> &str;
    fn setup(&mut self);
    fn update(&mut self, input: &AppInput) -> AppResult;          // touch, swipe, tap, accel, dt
    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D);
}
```

Simpler apps skip the trait and just expose `handle_tap(x, y)` + `render()`. The main loop:

- keeps one instance of each app alive (state survives leaving and re-entering),
- maps `AppState` (an enum, one variant per screen) to the right instance,
- does all the hardware work an app needs: IMU power, HTTP requests for the Home Assistant apps, panel brightness for the Flashlight, the alarm beeper, Wi-Fi for exports,
- chooses the tick rate per app, so a static screen costs almost nothing.

Adding an app means: a file in `src/apps/`, a variant in `AppState`, a row in the launcher's `MENU` list, and arms in the main loop's tick-budget and app `match` blocks.

## Networking and Home Assistant

- **Radio off by default.** Wi-Fi starts from the watch face toggle, Settings → CONNECT, a Home Assistant screen, or a Wi-Fi export, and turns itself off after 5 minutes idle.
- **Two networks**: the home network (`WIFI_SSID`) and an optional second one (`WIFI2_SSID`, e.g. a phone hotspot). Settings picks Auto (home first, then hotspot), home only, or hotspot only. Each attempt is capped at 8 s.
- **NTP** sets the RTC on connect (US Eastern with automatic DST).
- **HTTP client** (`http.rs`): plain HTTP/1.1 over embassy-net TCP, IPv4 literal hosts. Home Assistant is reached at `HA_HOST`:
  - *Smart Home* buttons POST to HA webhooks (no auth needed),
  - *Home Status*, *Weather* and *Fleet* POST a Jinja template to `/api/template` with the `HA_TOKEN` bearer token. HA renders a short text report, so all "which entities?" logic lives in the template, not the firmware.
- **Web export** (`webexport.rs`): a one-connection-at-a-time server on port 80 serving the battery log as text or CSV.

## Battery log

`peripherals/battlog.rs`: a ring of 16-byte records in raw flash.

- **Where**: 64 KB (16 × 4 KB sectors) at `0x00C0_0000`, the 12 MB mark of the 32 MB flash. The firmware image is ~0.9 MB at the start of flash and flashing never reaches this area.
- **Records**: a *normal* record (sequence, minute since 2000, mV, screen-on s, %, flags, Wi-Fi time) followed by a *diag* record (light-sleep s, awake s, timer wakes, touch wakes). Each has a magic nibble and XOR checksum, so leftover flash contents are never mistaken for data.
- **When**: every ~10 minutes, at every boot, and when the USB cable is plugged in or pulled out. Boot records carry reset reason, power-chip on/off source and the last breadcrumbs.
- **Wear**: a sector is erased only when the ring wraps onto it (about once a day). 4096 records ≈ two weeks of history.
- **State of charge** shown on battery is derived from voltage with an open-circuit-voltage table measured on the 400 mAh cell; the AXP2101's own % is used while charging.

## Diagnostics and recovery

- **Panic handler**: saves the panic message to persistent RTC memory and software-resets; the next boot shows it in the Battery app and on serial.
- **Breadcrumbs**: `crumb(n)` writes the current loop stage into a 16-entry ring in persistent RTC memory. After an unexpected reset the last steps are printed and logged.
- **Reset reason + power-chip cause**: logged on every boot (e.g. RTC watchdog, USB reset, PMU under-voltage).
- **RTC watchdog**: 10 minutes, fed every loop pass; a hung loop resets the chip.
- **I2C**: bus recovery at boot plus a 50 ms per-transaction timeout.
- Note: persistent RTC memory is cleared by esp-hal on a power-on reset, so crashes that end in a full power cycle can't leave a message.

## Memory

| Region | Use |
|---|---|
| Internal RAM heap | 192 KB: network stack, HTTP buffers, app state |
| Main task stack | what remains (~18 KB); large buffers go on the heap, not the stack |
| PSRAM (8 MB octal) | framebuffer (~402 KB) and large allocations |
| Persistent RTC fast memory | crash message, breadcrumb ring |
| Flash 0x00C0_0000 | battery log |

## Hardware quirks worth knowing

These shaped the code and are documented in comments where they're handled:

1. **Touch doesn't show in the wake-cause register** (GPIO38 isn't an RTC IO). Wakes are classified by timing instead (see [Light sleep](#light-sleep-and-waking)).
2. **The RTC slow-clock counter needs an update-and-wait** before reading. Read it immediately and you get the previous value, which also made esp-hal's sleep timer occasionally compute a wake time in the past.
3. **Don't touch I2C in the first moments after light sleep.** Reading the clock chip immediately after wake corrupted its time registers; the firmware now waits 5 ms and avoids I2C around the sleep call entirely.
4. **esp-println's auto mode picks the wrong console on the S3**; the `jtag-serial` feature is forced so USB serial works.
5. **Opening the port with DTR/RTS toggled resets the watch into download mode** (the browser esptool console does this). `tools/watch-serial.ps1` opens it with both lines held off.
6. **Plugging USB in while asleep** can enumerate as an unknown device; replug after the watch wakes.
7. **Powering down the ES7210 mic ADC raised idle current** instead of lowering it, so it is left alone.
8. **AXP2101 charger state** lives in register 0x01 bits 2:0 (with current direction in bits 6:5); reading the wrong bits shows "charging" on battery.
9. **Phone hotspots isolate clients**: a laptop on the same hotspot can't reach the watch, but the phone itself can.

## The screenshot renderer

`tools/preview` is a small host program that compiles the firmware's own UI and app modules (`#[path]` includes, no copies) against a PC framebuffer and writes PNGs. Hardware-only pieces (flash, display errors) are stubbed. The `gallery` mode renders every screen into `docs/screenshots`:

```sh
tools/preview/gallery.sh            # Linux
tools\preview\gallery.ps1           # Windows
```

Because it runs the real drawing code, it doubles as a quick way to iterate on layouts without flashing the watch.
