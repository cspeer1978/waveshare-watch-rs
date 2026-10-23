#![no_std]
#![no_main]

extern crate alloc;

mod board;
mod drivers;
mod peripherals;
mod ui;
mod apps;

use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use embedded_graphics_core::prelude::RawData;
use embedded_hal_bus::i2c::RefCellDevice;
use esp_alloc as _;
use esp_backtrace as _;

// ===== Crash capture =====
// A panic (including CPU exceptions and stack overflows, which esp-hal turns
// into panics) used to print to USB and then halt: on the wrist that looked
// like a dead watch that needed the 10-second power-button reset.
// Now the message is kept in RTC fast memory (survives a software reset) and
// the watch restarts by itself; at boot it is printed, shown in the Battery
// app and the restart reason goes into the battery log.
const CRASH_MAGIC: u32 = 0xC0FF_EE42;
const CRASH_CAP: usize = 240;
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut CRASH_TAG: u32 = 0;
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut CRASH_LEN: u32 = 0;
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut CRASH_MSG: [u8; CRASH_CAP] = [0; CRASH_CAP];

struct CrashWriter { n: usize }
impl core::fmt::Write for CrashWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let p = core::ptr::addr_of_mut!(CRASH_MSG) as *mut u8;
        for &b in s.as_bytes() {
            if self.n >= CRASH_CAP { break; }
            unsafe { p.add(self.n).write_volatile(if b == b'\n' { b' ' } else { b }) };
            self.n += 1;
        }
        Ok(())
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    use core::fmt::Write as _;
    let mut w = CrashWriter { n: 0 };
    // Location first ("main.rs:1234"), so the short on-screen version says where.
    if let Some(l) = info.location() {
        let file = l.file().rsplit('/').next().unwrap_or(l.file());
        let _ = write!(w, "{}:{} ", file, l.line());
    }
    let _ = write!(w, "{}", info.message());
    unsafe {
        core::ptr::addr_of_mut!(CRASH_LEN).write_volatile(w.n as u32);
        core::ptr::addr_of_mut!(CRASH_TAG).write_volatile(CRASH_MAGIC);
    }
    esp_println::println!("\n!!! PANIC: {}", info);
    esp_hal::system::software_reset()
}

// ===== Breadcrumbs =====
// The main loop writes a step number at each stage into RTC fast memory
// (survives a watchdog reset). After a hang + watchdog restart, boot prints
// the last 16 steps, i.e. exactly where the loop stopped.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut CRUMBS: [u8; 16] = [0; 16];
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut CRUMB_IDX: u32 = 0;
#[inline(always)]
/// The chip's own slow-clock counter (RTC_CNTL timer): the same counter the
/// light-sleep wake timer runs on, and it keeps counting while asleep.
/// esp-hal reads it right after asking for an update, which can return the
/// value from the *previous* update - that is why "time since boot" looked
/// frozen across sleep, and it can also put the 5-minute wake alarm in the
/// past (the watch then sleeps until a touch or BOOT). Here we ask for the
/// update and wait for it to land before reading. No I2C involved.
fn slow_ticks() -> u64 {
    unsafe {
        let u = core::ptr::read_volatile(0x6000_800C as *const u32);
        core::ptr::write_volatile(0x6000_800C as *mut u32, u | (1 << 31));
        esp_hal::delay::Delay::new().delay_micros(100); // >= several slow-clock periods
        let lo = core::ptr::read_volatile(0x6000_8010 as *const u32);
        let hi = core::ptr::read_volatile(0x6000_8014 as *const u32) & 0xFFFF;
        ((hi as u64) << 32) | lo as u64
    }
}

/// Nominal slow-clock rate: exactly what esp-hal uses to turn the sleep
/// duration into timer ticks (RTC_CNTL_CLK_CONF bits 31:30 pick the source).
fn slow_nominal_hz() -> u64 {
    match (unsafe { core::ptr::read_volatile(0x6000_8074 as *const u32) } >> 30) & 3 {
        1 => 32_768,            // 32 kHz crystal
        2 => 17_500_000 / 256,  // RC_FAST / 256
        _ => 136_000,           // internal RC (default)
    }
}

fn crumb(step: u8) {
    unsafe {
        let i = core::ptr::addr_of!(CRUMB_IDX).read_volatile();
        let p = core::ptr::addr_of_mut!(CRUMBS) as *mut u8;
        p.add((i % 16) as usize).write_volatile(step);
        core::ptr::addr_of_mut!(CRUMB_IDX).write_volatile(i.wrapping_add(1));
    }
}
/// Last 16 steps, oldest first.
fn crumb_trail() -> ([u8; 16], u32) {
    let mut out = [0u8; 16];
    unsafe {
        let i = core::ptr::addr_of!(CRUMB_IDX).read_volatile();
        let p = core::ptr::addr_of!(CRUMBS) as *const u8;
        for k in 0..16u32 { out[k as usize] = p.add(((i.wrapping_add(k)) % 16) as usize).read_volatile(); }
        (out, i)
    }
}

/// Message saved by a crash before the last restart (cleared once read).
fn take_crash_message() -> Option<alloc::string::String> {
    unsafe {
        if core::ptr::addr_of!(CRASH_TAG).read_volatile() != CRASH_MAGIC { return None; }
        core::ptr::addr_of_mut!(CRASH_TAG).write_volatile(0);
        let n = (core::ptr::addr_of!(CRASH_LEN).read_volatile() as usize).min(CRASH_CAP);
        let p = core::ptr::addr_of!(CRASH_MSG) as *const u8;
        let bytes = core::slice::from_raw_parts(p, n);
        Some(alloc::string::String::from_utf8_lossy(bytes).into_owned())
    }
}

esp_bootloader_esp_idf::esp_app_desc!();

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};

use esp_hal::delay::Delay;
use esp_hal::dma::{DmaRxBuf, DmaTxBuf};
use esp_hal::dma_buffers;
// use esp_hal::i2s::master::{I2s, Config as I2sConfig, DataFormat}; // TODO: wire I2S
use esp_hal::gpio::{InputConfig, Level, Output, OutputConfig, Pull, Input};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode as SpiMode;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_println::println;

use crate::drivers::co5300::Co5300Display;
use crate::drivers::framebuffer::Framebuffer;
use crate::drivers::qspi_bus::QspiBus;
use crate::peripherals::power::Axp2101Power;
use crate::peripherals::power_stats::{DisplayState, PowerStats, WifiMode};
use crate::peripherals::touch::{Ft3168Touch, SwipeDirection};
use crate::peripherals::rtc::{Pcf85063aRtc, DateTime};
use crate::peripherals::imu::Qmi8658Imu;
use crate::ui::watchface::WatchFace;
use crate::ui::pages::{self, Page};
use crate::ui::power_page;
use crate::apps::{App, AppInput, AppResult, AppState};
use crate::apps::tetris::TetrisGame;
use crate::apps::maze::MazeGame;
use crate::ui::launcher::Launcher;
use crate::apps::settings::SettingsApp;
use crate::apps::smarthome::{SmartHomeApp, HttpMethod};
use crate::apps::stopwatch::StopwatchApp;
use crate::apps::timer::TimerApp;
use crate::apps::flashlight::{self, FlashlightApp};
use crate::apps::steps::StepsApp;
use crate::apps::ha_info::{self, HaInfoApp, HaKind};
use crate::apps::alarm::{self, AlarmApp};
use crate::apps::calculator::CalculatorApp;
use crate::apps::level::LevelApp;
use crate::apps::dice::DiceApp;
use crate::apps::sunmoon::SunMoonApp;
use crate::apps::tictactoe::TicTacToeApp;
use crate::apps::asteroids::AsteroidsApp;
use crate::apps::wifiqr::WifiQrApp;
use crate::apps::fluid::FluidApp;
use crate::apps::waves::WavesApp;
use crate::apps::battery::BatteryApp;
use crate::peripherals::battlog::{self, BattLog, Entry};
use crate::peripherals::http;
use crate::peripherals::audio::{Es8311, fill_beep_buffer};

// Network runner task (must be spawned for WiFi to work)
#[embassy_executor::task]
async fn net_task(mut runner: embassy_net::Runner<'static, esp_radio::wifi::WifiDevice<'static>>) -> ! {
    runner.run().await
}

// Simple NTP sync (UDP to pool.ntp.org:123)
async fn ntp_sync(
    stack: embassy_net::Stack<'static>,
    rtc: &mut crate::peripherals::rtc::Pcf85063aRtc<impl embedded_hal::i2c::I2c>,
) -> Result<(), ()> {
    use embassy_net::udp::{UdpSocket, PacketMetadata};

    let mut rx_meta = [PacketMetadata::EMPTY; 1];
    let mut rx_buf = [0u8; 256];
    let mut tx_meta = [PacketMetadata::EMPTY; 1];
    let mut tx_buf = [0u8; 256];

    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    socket.bind(12345).map_err(|_| ())?;

    // NTP request packet (simplified: 48 bytes, first byte = 0x1B for client mode)
    let mut ntp_request = [0u8; 48];
    ntp_request[0] = 0x1B; // LI=0, VN=3, Mode=3 (client)

    // Resolve pool.ntp.org (use Google's NTP IP directly: 216.239.35.0)
    let ntp_addr = embassy_net::Ipv4Address::new(216, 239, 35, 0);
    socket.send_to(&ntp_request, (ntp_addr, 123)).await.map_err(|_| ())?;

    // Wait for response (timeout 5s)
    let mut response = [0u8; 48];
    match embassy_time::with_timeout(
        Duration::from_secs(5),
        socket.recv_from(&mut response),
    ).await {
        Ok(Ok((len, _addr))) if len >= 48 => {
            // Parse NTP timestamp (bytes 40-43 = seconds since 1900-01-01)
            let ntp_secs = u32::from_be_bytes([response[40], response[41], response[42], response[43]]);
            // Convert NTP epoch (1900) to Unix epoch (1970): subtract 70 years in seconds
            let unix_secs = ntp_secs.wrapping_sub(2_208_988_800);
            // Convert to hours/minutes/seconds in America/New_York.
            // EST = UTC-5, EDT = UTC-4; is_us_dst() picks the right one.
            let offset_secs: i32 = if is_us_dst(unix_secs) { -4 * 3600 } else { -5 * 3600 };
            let local_secs = (unix_secs as i64 + offset_secs as i64) as u32;
            let time_of_day = local_secs % 86400;
            let hours = (time_of_day / 3600) as u8;
            let minutes = ((time_of_day % 3600) / 60) as u8;
            let seconds = (time_of_day % 60) as u8;

            // Calculate date (simplified: days since epoch)
            let total_days = (local_secs / 86400) as i32;
            // Simple date from days since 1970-01-01
            let (year, month, day) = days_to_date(total_days);

            println!("[NTP] Time: {:02}:{:02}:{:02} {:02}/{:02}/{}", hours, minutes, seconds, day, month, year);

            // Set RTC
            let dt = crate::peripherals::rtc::DateTime::new(
                (year - 2000) as u8, month as u8, day as u8,
                hours, minutes, seconds,
            );
            let _ = rtc.set_time(&dt);
            Ok(())
        }
        _ => Err(()),
    }
}

fn days_to_date(days_since_epoch: i32) -> (u32, u32, u32) {
    // Simplified date calculation from days since 1970-01-01
    let mut y = 1970i32;
    let mut remaining = days_since_epoch;
    loop {
        let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
        if remaining < days_in_year { break; }
        remaining -= days_in_year;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let month_days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut m = 0;
    while m < 12 && remaining >= month_days[m] {
        remaining -= month_days[m];
        m += 1;
    }
    (y as u32, (m + 1) as u32, (remaining + 1) as u32)
}

/// True when US Eastern *daylight* time is in effect at the given UTC instant.
///
/// Rule in force since 2007: DST starts on the 2nd Sunday of March at 02:00
/// local standard time (07:00 UTC) and ends on the 1st Sunday of November at
/// 02:00 local daylight time (06:00 UTC).
fn is_us_dst(unix_secs: u32) -> bool {
    let days = (unix_secs / 86400) as i32;
    let secs_of_day = unix_secs % 86400;
    let (_year, month, day) = days_to_date(days);
    let (month, day) = (month as i32, day as i32);

    // Day 0 of the Unix epoch was a Thursday, so (days + 4) % 7 gives 0 = Sunday.
    // Step back to the 1st of the month to find which weekday it fell on.
    let wd_first = ((days - (day - 1)) + 4) % 7;
    let first_sunday = 1 + ((7 - wd_first) % 7);

    match month {
        1 | 2 | 12 => false,
        4..=10 => true,
        3 => {
            let second_sunday = first_sunday + 7;
            if day > second_sunday {
                true
            } else if day < second_sunday {
                false
            } else {
                secs_of_day >= 7 * 3600
            }
        }
        11 => {
            if day < first_sunday {
                true
            } else if day > first_sunday {
                false
            } else {
                secs_of_day < 6 * 3600
            }
        }
        _ => false,
    }
}

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::RgbColor;

/// Take a cheap snapshot of the current power state into the shared
/// `PowerStats` struct. Called from the Power page renderer (and from the
/// swipe preview) so the user sees a live read-out without us adding any
/// extra sampling. All inputs are already tracked by the main loop.
fn update_power_stats(
    stats: &mut PowerStats,
    screen_state: u8,
    imu_on: bool,
    wifi_connected: bool,
    wifi_on_request: bool,
    brightness: u8,
    batt_mv: u16,
    batt_pct: u8,
    charging: bool,
) {
    stats.display = Some(match screen_state {
        0 => DisplayState::Off,
        1 => DisplayState::Aod,
        2 => DisplayState::Dim,
        _ => DisplayState::Bright,
    });
    stats.wifi = Some(if !wifi_on_request && !wifi_connected {
        WifiMode::Off
    } else if wifi_connected {
        // set_power_save(Maximum) is applied via Config, so once connected
        // we're in WIFI_PS_MAX_MODEM.
        WifiMode::PowerSave
    } else {
        // Radio up but handshake still in progress.
        WifiMode::Active
    });
    stats.imu_on = imu_on;
    stats.brightness = brightness;
    // Audio/SD track "currently drawing current". The codec is in shutdown
    // except during a beep; the SD is not currently gated (see TODO in
    // main init). For now, report both off — flip these flags when the
    // main loop wakes the respective subsystem.
    stats.audio_on = false;
    stats.sd_on = false;
    stats.battery_mv = batt_mv;
    stats.battery_pct = batt_pct;
    stats.charging = charging;
}

/// Print the whole battery log as CSV on the USB serial console
/// (Battery app USB button, or "EXPORT" sent by the PC script).
fn print_battery_csv(entries: &[Entry]) {
    println!("--- BATTERY LOG CSV ---");
    println!("{}", crate::apps::battery::CSV_HEADER);
    for e in entries {
        println!("{}", crate::apps::battery::csv_line(e).as_str());
    }
    println!("--- END ({} records) ---", entries.len());
}

#[esp_rtos::main]
async fn main(_spawner: Spawner) {
    // Heap: 200KB SRAM + PSRAM for large allocs
    // The BLE+WiFi coex radio stack needs ~100KB+ of internal SRAM for
    // btdm_controller_init tasks and buffers. 64KB was too small and
    // caused a StoreProhibited panic (null-pointer from failed alloc).
    // 192 KB (was 200): leaves ~18 KB for the CPU stack, which was down to ~9 KB
    // and crashed at boot once the main task grew.
    esp_alloc::heap_allocator!(size: 192 * 1024);
    // Power-aware: default to 160MHz instead of 240MHz.
    // Saves ~30% CPU power without noticeable impact on UI/sensor work.
    // Game code can still trigger short bursts via DMA/peripherals at 80MHz QSPI which is unchanged.
    let mut peripherals = esp_hal::init(
        esp_hal::Config::default()
            .with_cpu_clock(esp_hal::clock::CpuClock::_160MHz)
    );

    // PSRAM
    esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::psram);

    // Embassy timer
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    esp_println::logger::init_logger_from_env();
    println!("=== Waveshare Watch RS v0.4 (Embassy) ===");

    let delay = Delay::new();

    // === I2C Bus ===
    // Bus recovery: if a chip was left mid-transfer by a reset (it then holds
    // SDA low and every read returns the same byte - the "2012-12-12 12:12"
    // dates in the battery log), clock SCL up to 9 times until it lets go,
    // then send a STOP. Standard I2C unjam procedure.
    {
        use esp_hal::gpio::DriveMode;
        let od = OutputConfig::default().with_drive_mode(DriveMode::OpenDrain).with_pull(Pull::Up);
        let mut scl = Output::new(peripherals.GPIO14.reborrow(), Level::High, od);
        let mut sda = Output::new(peripherals.GPIO15.reborrow(), Level::High, od).into_flex();
        sda.set_input_enable(true);
        delay.delay_micros(10);
        let mut pulses = 0;
        while !sda.is_high() && pulses < 9 {
            scl.set_low(); delay.delay_micros(5);
            scl.set_high(); delay.delay_micros(5);
            pulses += 1;
        }
        // STOP: SDA low -> high while SCL is high
        scl.set_low(); delay.delay_micros(5);
        sda.set_low(); delay.delay_micros(5);
        scl.set_high(); delay.delay_micros(5);
        sda.set_high(); delay.delay_micros(5);
        println!("[I2C] bus recovery: {} clock pulse(s), SDA {}", pulses, if sda.is_high() { "free" } else { "STILL LOW" });
    }
    let i2c = I2c::new(
        peripherals.I2C0,
        // Software timeout: a jammed bus used to block a read forever, which
        // froze the whole watch until the watchdog reset it.
        I2cConfig::default()
            .with_frequency(Rate::from_khz(400))
            .with_software_timeout(esp_hal::i2c::master::SoftwareTimeout::Transaction(esp_hal::time::Duration::from_millis(50))),
    )
    .expect("I2C failed")
    .with_sda(peripherals.GPIO15)
    .with_scl(peripherals.GPIO14);
    let i2c_ref = RefCell::new(i2c);

    // === Power ===
    let mut power = Axp2101Power::new(RefCellDevice::new(&i2c_ref));
    let _ = power.init();
    // Drop the die-temp ADC channel we never show on the UI — shaves a
    // few hundred µA off the AXP2101 housekeeping load.
    let _ = power.trim_adc_channels();
    println!("[POWER] OK");

    // === Display 80MHz DMA ===
    let spi_config = SpiConfig::default()
        .with_frequency(Rate::from_mhz(80))
        .with_mode(SpiMode::_0);
    let (rx_buf, rx_desc, tx_buf, tx_desc) = dma_buffers!(8000);
    let dma_rx = DmaRxBuf::new(rx_desc, rx_buf).unwrap();
    let dma_tx = DmaTxBuf::new(tx_desc, tx_buf).unwrap();
    let spi = Spi::new(peripherals.SPI2, spi_config)
        .expect("SPI failed")
        .with_sck(peripherals.GPIO11)
        .with_sio0(peripherals.GPIO4)
        .with_sio1(peripherals.GPIO5)
        .with_sio2(peripherals.GPIO6)
        .with_sio3(peripherals.GPIO7)
        .with_dma(peripherals.DMA_CH0)
        .with_buffers(dma_rx, dma_tx);
    let cs = Output::new(peripherals.GPIO12, Level::High, OutputConfig::default());
    let reset = Output::new(peripherals.GPIO8, Level::High, OutputConfig::default());
    let mut display = Co5300Display::new(QspiBus::new(spi, cs), reset);
    display.init();

    // Enable Tearing Effect output on CO5300 (TE pin = GPIO13)
    // Command 0x35 = TEARON, param 0x00 = VBlank only
    display.bus_mut().write_c8d8(0x35, 0x00);
    let te_pin = Input::new(peripherals.GPIO13, InputConfig::default());
    println!("[DISPLAY] OK (TE VSync enabled)");

    // === Framebuffer PSRAM ===
    let mut fb = Framebuffer::new();
    fb.clear_color(Rgb565::BLACK);
    fb.flush(&mut display);
    println!("[FB] OK");

    // === Touch ===
    let mut touch_rst = Output::new(peripherals.GPIO9, Level::High, OutputConfig::default());
    // GPIO38 is the FT3168 INT line: held high by pull-up, pulled low by the controller
    // when a finger is on the screen. We use it both for level checks and as an async wake source.
    let mut touch_int = Input::new(peripherals.GPIO38, InputConfig::default().with_pull(Pull::Up));
    touch_rst.set_low(); delay.delay_millis(10); touch_rst.set_high(); delay.delay_millis(50);
    let mut touch = Ft3168Touch::new(RefCellDevice::new(&i2c_ref));
    let _ = touch.init();
    println!("[TOUCH] OK");

    // === RTC ===
    let mut rtc = Pcf85063aRtc::new(RefCellDevice::new(&i2c_ref));
    let _ = rtc.init();
    println!("[RTC] OK");
    // Real rate of the slow clock (the internal RC is only roughly 136 kHz):
    // count its ticks over 250 ms of the crystal-timed delay.
    let slow_hz_nom = slow_nominal_hz();
    let slow_hz_real = {
        let a = slow_ticks();
        Delay::new().delay_millis(250);
        let b = slow_ticks();
        let hz = b.saturating_sub(a) * 4;
        if hz > 10_000 && hz < 400_000 { hz } else { slow_hz_nom }
    };
    println!("[SLOWCLK] nominal {} Hz, measured {} Hz", slow_hz_nom, slow_hz_real);

    // === IMU ===
    let mut imu = Qmi8658Imu::new(RefCellDevice::new(&i2c_ref));
    let _ = imu.init();
    println!("[IMU] OK");

    // === SD Card (SPI3) ===
    println!("[SD] Init...");
    let sd_spi_config = SpiConfig::default()
        .with_frequency(Rate::from_mhz(4))
        .with_mode(SpiMode::_0);
    let sd_spi = Spi::new(peripherals.SPI3, sd_spi_config)
        .expect("SPI3 failed")
        .with_sck(peripherals.GPIO2)
        .with_mosi(peripherals.GPIO1)
        .with_miso(peripherals.GPIO3);
    let sd_cs = Output::new(peripherals.GPIO17, Level::High, OutputConfig::default());

    use embedded_hal_bus::spi::ExclusiveDevice;
    let sd_spi_dev = ExclusiveDevice::new_no_delay(sd_spi, sd_cs).unwrap();
    let mut sd_card = embedded_sdmmc::SdCard::new(sd_spi_dev, delay);
    let mut sd_ok = false;
    let mut mp3_files: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
    match sd_card.num_bytes() {
        Ok(size) => {
            println!("[SD] Card {}MB", size / 1024 / 1024);

            // Scan /mp3/ directory
            struct DummyTime;
            impl embedded_sdmmc::TimeSource for DummyTime {
                fn get_timestamp(&self) -> embedded_sdmmc::Timestamp {
                    embedded_sdmmc::Timestamp::from_calendar(2026, 4, 6, 12, 0, 0).unwrap()
                }
            }

            let mut volume_mgr = embedded_sdmmc::VolumeManager::new(sd_card, DummyTime);
            match volume_mgr.open_raw_volume(embedded_sdmmc::VolumeIdx(0)) {
            Ok(volume) => {
                if let Ok(root_dir) = volume_mgr.open_root_dir(volume) {
                    if let Ok(mp3_dir) = volume_mgr.open_dir(root_dir, "MP3") {
                        println!("[SD] Found /MP3/ folder");
                        let _ = volume_mgr.iterate_dir(mp3_dir, |entry| {
                            if !entry.attributes.is_directory() {
                                let name = core::str::from_utf8(&entry.name.base_name()).unwrap_or("?");
                                let ext = core::str::from_utf8(&entry.name.extension()).unwrap_or("");
                                let full = alloc::format!("{}.{}", name.trim(), ext.trim());
                                println!("[SD]   {}", full);
                                mp3_files.push(full);
                            }
                        });
                        println!("[SD] {} files found", mp3_files.len());
                        let _ = volume_mgr.close_dir(mp3_dir);
                        sd_ok = true;
                    } else {
                        // Try lowercase
                        if let Ok(mp3_dir) = volume_mgr.open_dir(root_dir, "mp3") {
                            println!("[SD] Found /mp3/ folder");
                            let _ = volume_mgr.iterate_dir(mp3_dir, |entry| {
                                if !entry.attributes.is_directory() {
                                    let name = core::str::from_utf8(&entry.name.base_name()).unwrap_or("?");
                                    let ext = core::str::from_utf8(&entry.name.extension()).unwrap_or("");
                                    let full = alloc::format!("{}.{}", name.trim(), ext.trim());
                                    println!("[SD]   {}", full);
                                    mp3_files.push(full);
                                }
                            });
                            println!("[SD] {} files found", mp3_files.len());
                            let _ = volume_mgr.close_dir(mp3_dir);
                            sd_ok = true;
                        } else {
                            println!("[SD] No /mp3/ or /MP3/ folder");
                        }
                    }
                    let _ = volume_mgr.close_dir(root_dir);
                } else {
                    println!("[SD] Can't open root dir");
                }
            }
            Err(e) => {
                println!("[SD] Can't open volume: {:?}", e);
            }
            }
        }
        Err(_) => println!("[SD] No card"),
    }

    // === Audio (ES8311 codec + I2S) ===
    // CRITICAL ORDER:
    // 1. Keep PA amplifier DISABLED (GPIO46 LOW) - prevents white noise from floating I2S line
    // 2. Init codec (codec init leaves DAC powered but no input yet)
    // 3. Immediately mute the codec DAC + HP output
    // 4. Init I2S bus
    // Only when we actually beep: unmute codec -> raise PA_EN -> write DMA -> lower PA_EN -> mute
    println!("[AUDIO] Init codec...");
    let mut audio_codec = Es8311::new(RefCellDevice::new(&i2c_ref));
    let mut pa_en = Output::new(peripherals.GPIO46, Level::Low, OutputConfig::default());
    match audio_codec.init() {
        Ok(()) => println!("[AUDIO] Codec OK"),
        Err(_) => println!("[AUDIO] Codec FAILED"),
    }
    // Full power-down of the analog blocks (not just mute). The PGA, DAC
    // and HP driver are explicitly cut — saves ~20 mA versus mute() which
    // only zeroes the volume register. `unmute()` brings them back on
    // demand at playback time.
    let _ = audio_codec.shutdown();

    // The board's ES7210 microphone ADC (I2C 0x40) is never used: put it in
    // power-down (mic bias + PGAs off, clocks off, analog/digital power down;
    // same sequence as Espressif's es7210 stop). Harmless if it's not fitted.
    {
        use embedded_hal::i2c::I2c as _;
        let mut adc = RefCellDevice::new(&i2c_ref);
        let mut ok = true;
        // REVERTED: the power-down write sequence coincided with idle drain
        // going from ~25 mA to ~40 mA (Oct 21 log). The chip is left as it
        // powers up again (never initialised = never started); we only check
        // that it answers.
        let mut b = [0u8; 1];
        ok &= adc.write_read(0x40, &[0x06], &mut b).is_ok();
        println!("[AUDIO] ES7210 mic ADC: {} (left untouched, reg 0x06=0x{:02X})", if ok { "present" } else { "not found" }, b[0]);
    }
    // Which power-chip outputs are on (for the idle-current hunt).
    println!("[PMU] DC_ONOFF(0x80)=0x{:02X} LDO_ONOFF0(0x90)=0x{:02X} LDO_ONOFF1(0x91)=0x{:02X}",
        power.read_raw(0x80).unwrap_or(0xFF), power.read_raw(0x90).unwrap_or(0xFF), power.read_raw(0x91).unwrap_or(0xFF));

    // === I2S Audio Output (using public write_dma) ===
    println!("[AUDIO] Init I2S...");
    use esp_hal::i2s::master::{I2s, Config as I2sConfig, DataFormat};
    use esp_hal::dma::DmaDescriptor;
    let i2s_config = I2sConfig::default()
        .with_sample_rate(Rate::from_hz(16000))
        .with_data_format(DataFormat::Data16Channel16);
    let i2s_periph = I2s::new(peripherals.I2S0, peripherals.DMA_CH1, i2s_config)
        .expect("I2S failed")
        .with_mclk(peripherals.GPIO16);
    static mut I2S_TX_DESC: [DmaDescriptor; 8] = [DmaDescriptor::EMPTY; 8];
    let mut i2s_tx = i2s_periph.i2s_tx
        .with_bclk(peripherals.GPIO41)
        .with_ws(peripherals.GPIO45)
        .with_dout(peripherals.GPIO40)
        .build(unsafe { &mut I2S_TX_DESC });

    // Pre-generate beep sound (800Hz, 50ms, stereo 16-bit @ 16kHz = 3200 bytes)
    static mut BEEP_BUF: [u8; 4000] = [0u8; 4000];
    let beep_len = fill_beep_buffer(unsafe { &mut BEEP_BUF }, 800, 16000, 50);
    println!("[AUDIO] I2S OK ({} bytes beep)", beep_len);

    // === WiFi init (esp-radio) — RADIO STAYS OFF AT BOOT ===
    //
    // POWER BUDGET NOTE:
    // Up to v0.4 the watch booted straight into a connected WiFi STA with
    // no power-save configured. The 2.4 GHz radio + PA alone sustains
    // ~90 mA continuous — the single biggest drain by far, roughly half
    // of the total ~1-hour runtime the user complained about.
    //
    // We now keep the esp-radio stack *initialized* (so we can spin it up
    // at any time without re-allocating 300 KB of radio buffers) but we
    // never call `wifi_controller.start()` on boot. The user toggles the
    // radio on/off with the 'W' button in the top-left of the watchface.
    // When toggled on, we also immediately set PowerSave::MaxModem so the
    // radio sleeps between DTIM beacons (~20 mA average instead of 90).
    //
    // DHCP + NTP are deferred to the first wake of the radio, so a watch
    // that never toggles WiFi pays exactly zero network cost.
    println!("[RADIO] Init radio stack (WiFi OFF, BLE OFF)");
    static RADIO: StaticCell<esp_radio::Controller<'static>> = StaticCell::new();
    // Coerce &'static mut → &'static so both WiFi and BLE can share the controller.
    let radio_controller: &'static esp_radio::Controller<'static> =
        RADIO.init(esp_radio::init().expect("esp-radio init failed"));

    // === WiFi init (OFF by default) ===
    let wifi_config = esp_radio::wifi::Config::default()
        .with_power_save_mode(esp_radio::wifi::PowerSaveMode::Maximum);
    let (mut wifi_controller, wifi_interfaces) = esp_radio::wifi::new(
        radio_controller,
        peripherals.WIFI,
        wifi_config,
    ).expect("WiFi init failed");

    // === BLE: created lazily on the first BLE toggle ===
    // Creating the BleConnector enables the Bluetooth controller, which then
    // stays powered. Most of the time nobody uses BLE, so we don't pay for it
    // until the user actually taps the BLE toggle.
    let mut bt_periph = Some(peripherals.BT);
    let mut ble_connector: Option<esp_radio::ble::controller::BleConnector<'static>> = None;
    println!("[BLE] Controller not started (starts on first toggle)");

    // Pre-fill STA credentials so "toggle WiFi" just flips a bit later.
    // Falls back to empty strings if WIFI_SSID / WIFI_PASS are not set at
    // compile time — WiFi simply won't connect but the watch boots fine.
    use esp_radio::wifi::{ModeConfig, ClientConfig, AuthMethod};
    // Two networks, both from build-time env vars (nothing secret in the source):
    //   home    = WIFI_SSID  / WIFI_PASS
    //   hotspot = WIFI2_SSID / WIFI2_PASS
    // Settings picks Auto (home first, then hotspot) or one of them.
    fn wifi_net_config(net: u8) -> Option<ModeConfig> {
        let (ssid, pass) = if net == crate::apps::settings::MODE_HOTSPOT {
            (option_env!("WIFI2_SSID").unwrap_or(""), option_env!("WIFI2_PASS").unwrap_or(""))
        } else {
            (option_env!("WIFI_SSID").unwrap_or(""), option_env!("WIFI_PASS").unwrap_or(""))
        };
        if ssid.is_empty() { return None; }
        Some(ModeConfig::Client(ClientConfig::default()
            .with_ssid(alloc::string::String::from(ssid))
            .with_password(alloc::string::String::from(pass))
            .with_auth_method(if pass.is_empty() { AuthMethod::None } else { AuthMethod::WpaWpa2Personal })))
    }
    // Which network the controller is currently configured for (255 = none).
    let mut wifi_cfg_net: u8 = 255;
    if let Some(cfg) = wifi_net_config(crate::apps::settings::MODE_HOME) {
        wifi_controller.set_config(&cfg).expect("WiFi config failed");
        wifi_cfg_net = crate::apps::settings::MODE_HOME;
    } else {
        println!("[WIFI] No SSID configured");
    }
    // Network we're connected to (for Settings), and whether the last attempt failed.
    let mut wifi_net: u8 = 255;
    let mut wifi_failed = false;
    // NOTE: we do NOT call wifi_controller.start() here. The radio stays
    // fully idle until the user taps the WiFi button.

    // === Network Stack scaffolding (idle until radio starts) ===
    use embassy_net::{Config as NetConfig, StackResources};
    use static_cell::StaticCell;

    let net_config = NetConfig::dhcpv4(Default::default());
    static RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();
    let resources = RESOURCES.init(StackResources::new());

    let (stack, runner) = embassy_net::new(wifi_interfaces.sta, net_config, resources, 12345u64);

    // Spawn network runner task. It does nothing useful until the radio
    // has been started, but spawning it here avoids having to hot-spawn
    // a task from inside the main loop.
    _spawner.spawn(net_task(runner)).ok();

    let mut boot_button = Input::new(peripherals.GPIO0, InputConfig::default().with_pull(Pull::Up));
    // Low-power RTC controller: used for light sleep and as a clock that keeps
    // running while asleep (embassy's Instant pauses during light sleep).
    let mut lp_rtc = esp_hal::rtc_cntl::Rtc::new(peripherals.LPWR);
    println!("=== All systems GO! (Embassy async, WiFi OFF) ===");

    // === State ===
    let mut watchface = WatchFace::new();
    watchface.wifi_connected = false; // radio stays off until user taps the button
    let mut current_page = Page::Clock;
    // Live power-diagnostic snapshot, updated in the main loop and read
    // by the Power page renderer. Kept as plain POD so reading it is free.
    let mut power_stats = PowerStats::new();
    power_stats.cpu_mhz = 160;
    let mut app_state = AppState::Watchface;
    let mut tetris_game = TetrisGame::new();
    let mut maze_game = MazeGame::new();
    let mut launcher = Launcher::new();
    let mut settings_app = SettingsApp::new();
    let mut smarthome_app = SmartHomeApp::new();
    let mut stopwatch_app = StopwatchApp::new();
    let mut timer_app = TimerApp::new();
    let mut flashlight_app = FlashlightApp::new();
    let mut steps_app = StepsApp::new();
    let mut ha_status_app = HaInfoApp::new(HaKind::Status);
    let mut weather_app = HaInfoApp::new(HaKind::Weather);
    let mut fleet_app = HaInfoApp::new(HaKind::Fleet);
    // True when an HA screen switched Wi-Fi on by itself; it switches it back
    // off as soon as the fetch is done (or you leave), to save battery.
    let mut ha_wifi_auto = false;
    let mut alarm_app = AlarmApp::new();
    let mut calc_app = CalculatorApp::new();
    let mut level_app = LevelApp::new();
    let mut dice_app = DiceApp::new();
    let mut sunmoon_app = SunMoonApp::new();
    let mut ttt_app = TicTacToeApp::new();
    let mut asteroids_app = AsteroidsApp::new();
    let mut wifiqr_app = WifiQrApp::new();
    let mut fluid_app = FluidApp::new();
    let mut waves_app = WavesApp::new();
    let mut battery_app = BatteryApp::new();
    // Battery history log in flash (survives the battery dying and re-flashing).
    let mut bat_log = BattLog::new(esp_storage::FlashStorage::new(peripherals.FLASH));
    println!("battlog: ok={} ", bat_log.ok);

    // Why did we start? (power on, watchdog, crash, ...) Goes into the first
    // battery-log entry of this boot, and a crash message is shown in Battery.
    let crash_msg = take_crash_message();
    let reset_code: u8 = if crash_msg.is_some() { 0xFE } else {
        esp_hal::system::reset_reason().map(|r| r as u8).unwrap_or(0)
    };
    println!("[BOOT] reset reason: {:?} (code {})", esp_hal::system::reset_reason(), reset_code);
    // Steps: 1 loop top | 10/11 light sleep in/out | 12/13 idle wait in/out |
    // 20/21 IMU | 30 RTC read | 40 battery read, 42/43 log write |
    // 50/51 touch poll | 60/61 Wi-Fi connect | 70 app screen | 80/81 display on/off
    let (trail, count) = crumb_trail();
    // Reset code 1 can mean power-on, brown-out or super-watchdog on the S3.
    // RTC_CNTL_SWD_CONF (0x6000_80B4) bit 0 = super-watchdog reset flag;
    // RTC_CNTL_BROWN_OUT (0x6000_80E8) bit 31 = brown-out detected now.
    let swd_conf = unsafe { core::ptr::read_volatile(0x6000_80B4 as *const u32) };
    let bod = unsafe { core::ptr::read_volatile(0x6000_80E8 as *const u32) };
    let swd_flag = swd_conf & 1 != 0;
    println!("[BOOT] super-watchdog reset flag: {}  brown-out reg 0x{:08X} (det={})", swd_flag, bod, bod >> 31);
    // Was RTC memory kept? (a real power-on clears it; resets keep it)
    let rtc_mem_kept = count != 0;
    // TEST: brown-out detector -> report only, no reset. Every unexplained
    // restart so far is reset code 1 (power-on / brown-out / super-watchdog).
    // With the reset disabled, a brown-out shows up as a counted event in the
    // log instead of a restart: if restarts stop and brown-outs appear, that
    // was the cause. RTC_CNTL_BROWN_OUT 0x6000_80E8: bit 26/28 = reset enables.
    unsafe {
        let r = core::ptr::read_volatile(0x6000_80E8 as *const u32);
        core::ptr::write_volatile(0x6000_80E8 as *mut u32, r & !((1 << 26) | (1 << 28)));
        // clear any stale brown-out / glitch / super-watchdog interrupt flags
        core::ptr::write_volatile(0x6000_804C as *mut u32, (1 << 9) | (1 << 15) | (1 << 19));
    }
    println!("[BOOT] brown-out reset disabled for this test (reg now 0x{:08X})", unsafe { core::ptr::read_volatile(0x6000_80E8 as *const u32) });
    println!("[BOOT] last loop steps before restart (oldest first, {} total): {:?}", count, trail);
    let last_step = trail[15];
    if reset_code == 16 || reset_code == 0xFE {
        let mut t: crate::apps::widgets::SBuf<36> = crate::apps::widgets::SBuf::new();
        let _ = core::fmt::Write::write_fmt(&mut t, format_args!("{} at step {}", if reset_code == 16 { "Watchdog" } else { "Crash" }, last_step));
        if crash_msg.is_none() { battery_app.set_crash(t.as_str()); }
    }
    // AXP2101 power-on / power-off source registers (0x20 / 0x21): why the
    // power chip last switched the watch on and off. 0x21 bits:
    //   0 PWR held for off-level, 1 software off, 2 PWR long-press off,
    //   3 VSYS under-voltage, 4 VBUS over-voltage, 5 DCDC under-voltage,
    //   6 DCDC over-voltage, 7 die over-temperature.
    let pmu_on = power.read_raw(0x20).unwrap_or(0xFF);
    let pmu_off = power.read_raw(0x21).unwrap_or(0xFF);
    let pmu_status1 = power.read_raw(0x00).unwrap_or(0xFF); // bit 3 = battery present
    println!("[PMU] power-on source 0x20=0x{:02X}  power-off source 0x21=0x{:02X}  status 0x00=0x{:02X}", pmu_on, pmu_off, pmu_status1);
    // PWR button hold time before the power chip switches the watch off
    // (REG 0x27 bits 3:2: 00=4 s, 01=6 s, 10=8 s, 11=10 s). The log showed the
    // watch being turned off by the button being held while handling it, so
    // use the longest: only a deliberate 10-second hold powers it off.
    {
        let r27 = power.read_raw(0x27).unwrap_or(0xFF);
        if r27 != 0xFF {
            let secs = [4, 6, 8, 10][((r27 >> 2) & 0x03) as usize];
            let _ = power.write_raw(0x27, (r27 & 0xF3) | (0b11 << 2));
            let now = power.read_raw(0x27).unwrap_or(0xFF);
            println!("[PMU] PWR hold-to-off was {} s (0x27=0x{:02X}), now 10 s (0x27=0x{:02X})", secs, r27, now);
        }
    }
    {
        let names = ["PWR held", "software off", "PWR long-press", "VSYS under-voltage", "VBUS over-voltage", "DCDC under-voltage", "DCDC over-voltage", "over-temperature"];
        for (i, n) in names.iter().enumerate() {
            if pmu_off != 0xFF && pmu_off & (1 << i) != 0 { println!("[PMU] last power-off cause: {}", n); }
        }
    }
    if let Some(m) = &crash_msg {
        println!("[BOOT] restarted after a crash: {}", m);
        battery_app.set_crash(m);
    }
    // Watchdog: if the main loop ever stops for 10 minutes (a hang, not a
    // panic), reset the chip instead of sitting there dead. The loop feeds it
    // every pass; light sleep is at most 5 minutes.
    lp_rtc.rwdt.set_stage_action(esp_hal::rtc_cntl::RwdtStage::Stage0, esp_hal::rtc_cntl::RwdtStageAction::ResetSystem);
    lp_rtc.rwdt.set_timeout(esp_hal::rtc_cntl::RwdtStage::Stage0, esp_hal::time::Duration::from_secs(600));
    lp_rtc.rwdt.enable();
    lp_rtc.rwdt.feed();
    // Latest RTC reading (h, m, s, day, month, year) for the Big Clock page / Alarm.
    let mut clk: (u8, u8, u8, u8, u8, u8) = (0, 0, 0, 1, 1, 26);
    if let Ok(dt) = rtc.get_time() { clk = (dt.hours, dt.minutes, dt.seconds, dt.day, dt.month, dt.year); }
    let mut bigclock_drawn: u16 = 0xFFFF; // minute-of-day last drawn on the Big Clock page
    // Alarm bookkeeping
    let mut next_alarm_check = Instant::now();
    let mut alarm_next_beep = Instant::now();
    let mut alarm_ring_start = Instant::now();
    let mut ha_wait_since = Instant::now();
    // Flashlight keeps the screen awake for a while after the last tap.
    let mut flash_last_tap = Instant::now();
    let mut launcher_needs_draw = true;
    let mut last_touch_y: u16 = 0;
    let mut last_touch_x: u16 = 0;
    let mut accel = (0.0f32, 0.0f32, 0.0f32);
    let mut gyro_data = (0i16, 0i16, 0i16);
    let mut imu_temp: i16 = 250;
    let mut batt_pct: u8 = 0;
    let mut batt_mv: u16 = 0;
    let mut charging = false;
    let mut page_dirty = true;
    let mut swiping = false;
    let mut last_interaction = Instant::now();
    // screen_state levels:
    //   3 = full bright (interactive)
    //   2 = dim (transition)
    //   1 = AOD (Always-On Display: super-dim, minimal HH:MM, 1 update / minute)
    //   0 = full off (DISPOFF + SLPIN)
    let mut screen_state: u8 = 3;
    // Tracks the last minute we rendered in AOD so we update the screen exactly
    // once per minute, not faster. Saves both DMA bandwidth and AMOLED current.
    let mut aod_last_minute: u8 = 99;
    let mut swipe_dir: i32 = 0;
    let mut swipe_start_x: i32 = 0;
    let pixel_count = board::LCD_WIDTH as usize * board::LCD_HEIGHT as usize;
    let mut snap_current: Vec<u16> = vec![0u16; pixel_count];
    let mut snap_target: Vec<u16> = vec![0u16; pixel_count];

    // Initial render
    if let Ok(pct) = power.get_battery_percent() {
        batt_pct = pct;
        batt_mv = power.get_battery_voltage().unwrap_or(0);
        charging = power.is_charging().unwrap_or(false);
        watchface.update_battery(batt_pct, batt_mv, charging);
    }
    if let Ok(dt) = rtc.get_time() {
        watchface.update_time(dt.hours, dt.minutes, dt.seconds);
        watchface.update_date(dt.day, dt.month, dt.year);
    }
    watchface.force_redraw();
    let _ = watchface.render(&mut fb);
    fb.flush(&mut display);

    // === Event-driven async main loop ===
    //
    // The loop sleeps the CPU between iterations using `select` over:
    //   * a periodic timer whose period depends on the current app/screen state
    //   * the touch interrupt line (GPIO38, async falling edge)
    //   * the boot button line (GPIO0, async falling edge)
    //
    // Tick budgets per state (only consumed when nothing else wakes us):
    //   * screen off                : 30 s   (deep idle, just to refresh battery%)
    //   * watchface clock, gyro off : 1 s    (only the seconds digit changes)
    //   * watchface clock, gyro on  : 33 ms  (smooth gyro ball)
    //   * sensors page              : 100 ms (10 Hz IMU)
    //   * power page                : 1 s    (diagnostic — must not self-skew)
    //   * launcher / settings / smart home : 100 ms
    //   * Tetris / Maze : 33 ms (~30 Hz, panel cap)
    //
    // A finger held on the screen forces 16 ms tick regardless of state.
    //
    // When the user touches the screen or presses BOOT, the select returns immediately
    // and we process the input. With this design, the CPU is parked >99% of the time
    // while sitting on the watchface.
    use embassy_futures::select::select3;

    let mut next_rtc = Instant::now();
    let mut next_battery = Instant::now();
    let mut last_frame = Instant::now();
    let mut next_watchface_flush = Instant::now();
    // Radio state: we track both what the user *wants* and what the radio
    // actually is. They drift apart briefly during connect/disconnect.
    let mut wifi_on_request: bool = false;      // user toggle
    let mut wifi_started: bool = false;         // controller.start() called
    let mut wifi_connected: bool = false;       // connect_async succeeded
    let mut ntp_synced: bool = false;
    let mut last_ntp_attempt: Option<Instant> = None;
    let mut last_wifi_idle_check = Instant::now();
    // Request pending from a UI tap on the WiFi button.
    let mut wifi_toggle_request: bool = false;
    // BLE state
    let mut ble_on: bool = false;
    let mut ble_toggle_request: bool = false;
    // Power-down the IMU at boot — only enable when a consumer (gyro toggle, game, sensors page) needs it.
    let _ = imu.power_down();
    let mut imu_powered = false;
    let mut imu_mode: u8 = 0;
    // Measured battery drain: (battery %, RTC seconds) when we started measuring on battery.
    // Uses the RTC clock because embassy's Instant stops during light sleep.
    let mut drain_base: Option<(u8, u64)> = None;
    // Light sleep bookkeeping (for the Power page): ms spent asleep since unplugging.
    let mut slept_ms: u64 = 0;
    // Set when light sleep ended because of the touch panel or BOOT button.
    let mut woke_by_user = false;
    // USB cable present? We never light-sleep on USB so flashing/serial keep working.
    let mut usb_present = power.is_vbus_in().unwrap_or(true);
    // Battery log bookkeeping: screen-on / Wi-Fi-on time since the last record,
    // when (RTC minute) and in which USB state the last record was written.
    let mut log_screen_ms: u32 = 0;
    let mut log_radio_ms: u32 = 0;
    let mut log_awake_ms: u32 = 0;   // awake with the screen off (CPU running / waiting)
    let mut log_timer_wakes: u8 = 0; // light-sleep wakes by the 5-min timer
    let mut log_gpio_wakes: u8 = 0;  // light-sleep wakes by touch / BOOT
    let mut log_brownouts: u8 = 0;   // brown-out / glitch events seen (test build)
    let mut log_sleep_ms: u64 = 0;   // light sleep since the last record (slow-clock counter)
    let mut last_bo_step: u8 = 0;    // loop step when the last one was seen
    let mut last_log_min: Option<u32> = None; // None = nothing logged since boot
    let mut last_log_abs_s: Option<u64> = None; // RTC seconds at the last entry
    let mut last_log_ext: bool = false;       // USB/charging at the last record
    let mut serial_cmd = [0u8; 16];          // USB serial command line being received
    let mut serial_cmd_len = 0usize;
    let mut log_saves: u32 = 0;               // bumps on every saved record (Battery app reloads)
    let mut battery_app_saves: u32 = 0;       // log_saves value the Battery app last loaded
    let mut next_battery_live = Instant::now(); // Battery app: live reading refresh
    let mut next_vbus_check = Instant::now();  // quick plug/unplug detection while awake
    // Battery app Wi-Fi export: waiting for the network to come up?
    let mut bat_wifi_wait: Option<Instant> = None;
    let mut bat_wifi_auto = false;             // we turned Wi-Fi on just for the export
    let mut steps_dirty = true;
    // Tracks the previous-iteration state of the FT3168 INT line.
    // We need this to keep polling touch.poll() ONCE more after the finger lifts,
    // otherwise we miss the swipe-end event and pages stay stuck mid-drag.
    let mut was_touching = false;

    // Short beep burst on the speaker. Same order as the old Snake beep:
    // unmute codec -> amp on -> play -> amp off -> mute (no pops, amp off when idle).
    macro_rules! beep_burst {
        ($n:expr) => {{
            let _ = audio_codec.unmute();
            delay.delay_millis(2);
            for _ in 0..$n {
                pa_en.set_high();
                if let Ok(transfer) = i2s_tx.write_dma(unsafe { &BEEP_BUF }) {
                    let _ = transfer.wait();
                }
                pa_en.set_low();
                Timer::after(Duration::from_millis(90)).await;
            }
            // Full analog power-down (mute() alone left the codec's analog
            // section powered after the first beep, for the rest of the boot).
            let _ = audio_codec.shutdown();
        }};
    }

    loop {
        lp_rtc.rwdt.feed();
        crumb(1);
        // Brown-out / glitch / super-watchdog interrupt flags (RTC_CNTL_INT_RAW
        // 0x6000_8044: bit 9 brown-out, 15 super-watchdog, 19 glitch).
        {
            let raw = unsafe { core::ptr::read_volatile(0x6000_8044 as *const u32) };
            // Bit 15 (super-watchdog) is set after every light sleep: the SWD is
            // auto-fed, so it's harmless - clear it without counting it.
            if raw & (1 << 15) != 0 { unsafe { core::ptr::write_volatile(0x6000_804C as *mut u32, 1 << 15); } }
            let hit = raw & ((1 << 9) | (1 << 19));
            if hit != 0 {
                unsafe { core::ptr::write_volatile(0x6000_804C as *mut u32, hit); }
                log_brownouts = log_brownouts.saturating_add(1);
                last_bo_step = crumb_trail().0[14];
                println!("[POWER] event flags 0x{:X} (bit9 brown-out, 15 swd, 19 glitch) after step {}", hit, last_bo_step);
            }
        }
        // Pick a tick budget based on current state. This is the MAX time we'll
        // sleep without any external wake source.
        let touch_held = touch_int.is_low();
        let button_held = boot_button.is_low();

        let tick = if touch_held || button_held {
            // Something is currently being held: wake fast enough to track motion / detect
            // long-press, but no faster than necessary.
            Duration::from_millis(16) // ~60 Hz
        } else if app_state == AppState::Steps && steps_app.is_counting() {
            // Step counter: ~20 Hz accelerometer sampling, even with the screen
            // off. Only while the Steps app is open and counting.
            Duration::from_millis(50)
        } else if screen_state == 0 {
            // Screen completely off: only wake every 30 s for housekeeping (battery refresh).
            // GPIO falling edges still wake us instantly.
            Duration::from_secs(30)
        } else if screen_state == 1 {
            // AOD mode: wake every 10 s to check if a new minute has started.
            // We don't need exactly 60 s precision because the user only sees minutes change.
            Duration::from_secs(10)
        } else {
            match app_state {
                AppState::Watchface => match current_page {
                    // Clock page: 1 Hz when gyro is off (only seconds change),
                    // 33 ms when gyro is on (smooth ball animation).
                    Page::Clock => if watchface.gyro_enabled {
                        Duration::from_millis(33)
                    } else {
                        Duration::from_secs(1)
                    },
                    Page::Sensors => Duration::from_millis(100), // 10 Hz IMU display
                    Page::System  => Duration::from_secs(2),     // basically static
                    Page::BigClock => Duration::from_secs(2),    // redraws only when the minute changes
                    // Power page refreshes at 1 Hz — fast enough to see
                    // changes, slow enough not to skew the measurement.
                    Page::Power   => Duration::from_secs(1),
                },
                // ~60 Hz only while the list is moving; idle list = 100 ms.
                AppState::Launcher if launcher.is_animating() => Duration::from_millis(16),
                AppState::Launcher | AppState::Settings
                | AppState::SmartHome => Duration::from_millis(100),
                AppState::Tetris | AppState::Maze => Duration::from_millis(33),
                // Utility apps: fast ticks only while something is moving on screen.
                AppState::Stopwatch => if stopwatch_app.is_running() {
                    Duration::from_millis(100)
                } else {
                    Duration::from_secs(1)
                },
                AppState::Timer => if timer_app.is_running() {
                    Duration::from_millis(250)
                } else {
                    Duration::from_secs(1)
                },
                AppState::Flashlight => Duration::from_secs(1),
                AppState::Steps => Duration::from_millis(250),
                AppState::Battery => if bat_wifi_wait.is_some() { Duration::from_millis(200) } else { Duration::from_secs(5) },
                // HA screens: quick ticks only while connecting/loading.
                AppState::HaStatus | AppState::Weather | AppState::Fleet => {
                    let busy = match app_state {
                        AppState::HaStatus => ha_status_app.needs_fetch(),
                        AppState::Weather => weather_app.needs_fetch(),
                        _ => fleet_app.needs_fetch(),
                    };
                    if busy { Duration::from_millis(200) } else { Duration::from_secs(2) }
                }
                AppState::Alarm => if alarm_app.is_ringing() { Duration::from_millis(250) } else { Duration::from_secs(1) },
                AppState::Calculator | AppState::TicTacToe => Duration::from_secs(1),
                AppState::SunMoon | AppState::WifiQr => Duration::from_secs(30), // static screens
                AppState::Level | AppState::Dice => Duration::from_millis(50),
                AppState::Asteroids => Duration::from_millis(33),
                AppState::Fluid | AppState::Waves => Duration::from_millis(16), // as fast as the sim allows
            }
        };
        // A running countdown timer must wake us exactly when it ends,
        // whatever the screen is doing.
        let tick = match timer_app.remaining_ms() {
            Some(rem) => tick.min(Duration::from_millis(rem + 5)),
            None => tick,
        };
        // On USB: check for serial commands at least once a second (power is free).
        let tick = if usb_present { tick.min(Duration::from_secs(1)) } else { tick };

        // Sleep until the tick budget elapses OR a falling edge arrives on touch / boot button.
        // Notes:
        //   * If a pin is already low, wait_for_falling_edge will not fire (no edge to wait for),
        //     but the tick above is short (16 ms) so we still wake reactively.
        //   * The futures from esp-hal install GPIO interrupts on creation and remove them on
        //     drop, so the executor parks the CPU between events: this is the main power win.
        // === Light sleep (screen off, nothing running) ===
        // The big battery win: instead of idling at full clock (~100 mA measured),
        // the chip light-sleeps (a few mA) until a touch, the BOOT button, or a
        // 5-minute housekeeping timer. Only when it's safe:
        //   * screen fully off, nothing held down
        //   * Wi-Fi and Bluetooth off (radios can't sleep like this)
        //   * no running timer / stopwatch / step count (they use the embassy
        //     clock, which pauses during light sleep)
        //   * not on USB, so flashing and the serial log keep working
        let can_sleep = screen_state == 0
            && !touch_held && !button_held
            && !wifi_started && !wifi_on_request
            && ble_connector.is_none()
            && !timer_app.is_running()
            && !stopwatch_app.is_running()
            && !alarm_app.is_ringing()
            && !(app_state == AppState::Steps && steps_app.is_counting())
            && !usb_present;
        if can_sleep {
            use esp_hal::gpio::WakeEvent;
            use esp_hal::rtc_cntl::sleep::{GpioWakeupSource, TimerWakeupSource};
            let _ = touch_int.wakeup_enable(true, WakeEvent::LowLevel);
            let _ = boot_button.wakeup_enable(true, WakeEvent::LowLevel);
            // Time the sleep with the chip's own slow-clock counter - the same
            // counter the wake timer uses, so a timer wake measures exactly the
            // planned ticks and a touch/BOOT wake measures less. (The touchfix
            // build read the PCF85063 clock chip over I2C right before and right
            // after sleep; that scrambled the clock chip's time overnight.)
            // Wake at least every 5 min for housekeeping, or exactly at the alarm.
            let mut planned_s: u64 = 300;
            if alarm_app.is_armed() {
                if let Ok(dt) = rtc.get_time() {
                    if let Some(sec) = alarm_app.secs_until(dt.hours, dt.minutes, dt.seconds) {
                        planned_s = (sec as u64).clamp(1, 300);
                    }
                }
            }
            let timer_src = TimerWakeupSource::new(core::time::Duration::from_secs(planned_s));
            let gpio_src = GpioWakeupSource::new();
            crumb(10);
            // Fresh counter value right before sleeping: esp-hal reads the counter
            // without waiting when it sets the wake alarm, so make sure what it
            // reads is current (a stale value puts the alarm in the past).
            let t0 = slow_ticks();
            lp_rtc.sleep_light(&[&timer_src, &gpio_src]);
            crumb(11);
            // Let clocks and pads settle before anything touches I2C again.
            Delay::new().delay_millis(5);
            let t1 = slow_ticks();
            let slept_ticks = t1.saturating_sub(t0);
            let planned_ticks = planned_s * slow_hz_nom;
            // Wake-cause register (0x6000_8130: bit 2 = GPIO, bit 3 = timer). It
            // misses touch-panel wakes, so it is only a fallback.
            let cause = unsafe { core::ptr::read_volatile(0x6000_8130 as *const u32) } & 0x1_FFFF;
            let by_gpio = cause & (1 << 2) != 0;
            let by_timer = cause & (1 << 3) != 0;
            // Woke clearly (> 3 s) before the planned ticks = touch or BOOT.
            let early = if slept_ticks == 0 { !by_timer } else { slept_ticks + 3 * slow_hz_nom < planned_ticks };
            log_sleep_ms = log_sleep_ms.saturating_add(slept_ticks * 1000 / slow_hz_real);
            if !early && !by_gpio {
                log_timer_wakes = log_timer_wakes.saturating_add(1);
                slept_ms += slept_ticks * 1000 / slow_hz_real;
            } else {
                // Touch panel or BOOT (the touch chip only sends short pulses,
                // often over by the time we look, so don't wait for the pin).
                woke_by_user = true;
                log_gpio_wakes = log_gpio_wakes.saturating_add(1);
            }
            let _ = touch_int.wakeup_enable(false, WakeEvent::LowLevel);
            let _ = boot_button.wakeup_enable(false, WakeEvent::LowLevel);
            // The embassy clock didn't advance while asleep, so the 10-min battery
            // poll would never come due. Refresh it on each (rare) wake instead.
            next_battery = Instant::now();
            next_alarm_check = Instant::now();
        } else {
            crumb(12);
            let _ = select3(
                Timer::after(tick),
                touch_int.wait_for_falling_edge(),
                boot_button.wait_for_falling_edge(),
            ).await;
            crumb(13);
        }

        let now = Instant::now();
        let dt_ms = (now - last_frame).as_millis() as u32;
        last_frame = now;
        // Battery log: how long the screen (bright or dim) and Wi-Fi were on.
        if screen_state >= 2 { log_screen_ms = log_screen_ms.saturating_add(dt_ms); }
        else { log_awake_ms = log_awake_ms.saturating_add(dt_ms); }
        if wifi_started { log_radio_ms = log_radio_ms.saturating_add(dt_ms); }

        crumb(20);
        // === Sensors (gated by need + screen state) ===
        // IMU only when an interactive consumer needs it (gyro enabled, IMU-driven game, sensors page).
        // When screen is off OR no consumer needs it, we power-down the IMU completely
        // (CTRL7 = 0). The QMI8658's gyro alone draws ~1.5 mA so this is a meaningful win.
        let need_imu = screen_state >= 2
            && (watchface.gyro_enabled
                || app_state == AppState::Maze
                || app_state == AppState::Tetris
                || app_state == AppState::Level
                || app_state == AppState::Dice
                || app_state == AppState::Asteroids
                || app_state == AppState::Fluid
                || app_state == AppState::Waves
                || (app_state == AppState::Watchface && current_page == Page::Sensors));
        // Step counter needs only the accelerometer, and also with the screen off.
        let need_accel_only = !need_imu
            && app_state == AppState::Steps && steps_app.is_counting();
        // imu_mode: 0 = off, 1 = accel only, 2 = accel + gyro
        let want_mode: u8 = if need_imu { 2 } else if need_accel_only { 1 } else { 0 };
        if want_mode != imu_mode {
            let _ = match want_mode {
                2 => imu.power_up(),
                1 => imu.power_up_accel_only(),
                _ => imu.power_down(),
            };
            imu_mode = want_mode;
        }
        imu_powered = imu_mode != 0;
        if need_accel_only {
            if let Ok(a) = imu.read_accel() {
                if steps_app.feed(a.x, a.y, a.z) {
                    steps_dirty = true;
                }
            }
        }
        if need_imu {
            if let Ok(a) = imu.read_accel() {
                accel = (a.x, a.y, a.z);
                watchface.update_accel(a.x, a.y, a.z);
            }
            if let Ok(g) = imu.read_gyro() {
                gyro_data = ((g.x * 10.0) as i16, (g.y * 10.0) as i16, (g.z * 10.0) as i16);
            }
            if let Ok(t) = imu.read_temperature() {
                imu_temp = (t * 10.0) as i16;
            }
        }

        // RTC: 1 Hz update is enough for a clock display. Skip when screen is off OR in AOD
        // (AOD updates the RTC manually once per minute).
        crumb(21);
        if screen_state >= 2 && now >= next_rtc {
            crumb(30);
            if let Ok(dt) = rtc.get_time() {
                watchface.update_time(dt.hours, dt.minutes, dt.seconds);
                watchface.update_date(dt.day, dt.month, dt.year);
                clk = (dt.hours, dt.minutes, dt.seconds, dt.day, dt.month, dt.year);
            }
            next_rtc = now + Duration::from_secs(1);
        }

        // Battery: every 60 s normally, every 5 min when the screen is off
        // (we still check occasionally to track charge state and update on next wake).
        // USB serial commands from the PC (watch-serial.ps1 sends "EXPORT"):
        // read the USB-Serial-JTAG receive FIFO directly (EP1_CONF bit 2 = data
        // available, EP1 = next byte). Only while on USB.
        if usb_present {
            const EP1: *const u32 = 0x6003_8000 as *const u32;
            const EP1_CONF: *const u32 = 0x6003_8004 as *const u32;
            let mut n = 0;
            while n < 64 && unsafe { core::ptr::read_volatile(EP1_CONF) } & (1 << 2) != 0 {
                let b = (unsafe { core::ptr::read_volatile(EP1) } & 0xFF) as u8;
                n += 1;
                if b == b'\n' || b == b'\r' {
                    if &serial_cmd[..serial_cmd_len] == b"EXPORT" {
                        println!("[SERIAL] EXPORT requested");
                        match &crash_msg {
                            Some(m) => println!("[LAST CRASH] {}", m),
                            None => println!("[LAST CRASH] none since this boot started (reset code {})", reset_code),
                        }
                        println!("[PMU] at this boot: power-on source 0x{:02X}, power-off source 0x{:02X}", pmu_on, pmu_off);
                        println!("[BOOT] steps before the restart that started this boot: {:?}", trail);
                        print_battery_csv(&bat_log.read_all());
                    } else if serial_cmd_len > 0 {
                        println!("[SERIAL] unknown command (try EXPORT)");
                    }
                    serial_cmd_len = 0;
                } else if serial_cmd_len < serial_cmd.len() {
                    serial_cmd[serial_cmd_len] = b.to_ascii_uppercase();
                    serial_cmd_len += 1;
                }
            }
        }
        // Plug / unplug: notice within 5 s while the screen is on, so the battery
        // log records the moment (instead of at the next 3-minute battery read).
        if screen_state >= 2 && now >= next_vbus_check {
            if let Ok(v) = power.is_vbus_in() {
                if v != usb_present { next_battery = now; }
            }
            next_vbus_check = now + Duration::from_secs(5);
        }
        if now >= next_battery {
            crumb(40);
            if let Ok(pct) = power.get_battery_percent() {
                batt_pct = pct;
                batt_mv = power.get_battery_voltage().unwrap_or(0);
                charging = power.is_charging().unwrap_or(false);
                watchface.update_battery(batt_pct, batt_mv, charging);
                // Real drain rate from the AXP2101 fuel gauge (the Power page's
                // mA figures are only estimates). Needs >= 20 min on battery.
                let on_usb = power.is_vbus_in().unwrap_or(false);
                // (The USB "drop and restore D+ pull-up" re-attach that was here
                // left USB dead until a restart - removed. Plugging in while
                // asleep: wait for the next wake (<= 5 min), then replug once.)
                usb_present = on_usb;
                // Seconds clock that keeps counting in light sleep (PCF85063).
                let rtc_s = rtc.get_time().map(|d| battlog::rtc_minute(d.year, d.month, d.day, d.hours, d.minutes) as u64 * 60 + d.seconds as u64).unwrap_or(0);
                if charging || on_usb {
                    drain_base = None;
                    power_stats.drain_x10 = None;
                    power_stats.sleep_pct = None;
                    slept_ms = 0;
                } else {
                    match drain_base {
                        None => { drain_base = Some((batt_pct, rtc_s)); slept_ms = 0; }
                        Some((p0, t0)) => {
                            let secs = rtc_s.saturating_sub(t0);
                            if secs >= 20 * 60 && p0 > batt_pct {
                                let rate = ((p0 - batt_pct) as u64 * 36_000 / secs) as u16; // %/h x10
                                power_stats.drain_x10 = Some(rate.max(1));
                            }
                            if secs >= 60 {
                                power_stats.sleep_pct = Some((slept_ms / 10 / secs).min(100) as u8);
                            }
                        }
                    }
                }
                // Battery log: one record about every 10 minutes, plus one at boot
                // and whenever the cable is plugged in or pulled out.
                if let Ok(dt) = rtc.get_time() {
                    let minute = battlog::rtc_minute(dt.year, dt.month, dt.day, dt.hours, dt.minutes);
                    // Light-sleep time for the log = wall-clock time since the last
                    // entry (RTC chip, read here while fully awake) minus the time
                    // the loop was running (screen on + awake with screen off).
                    let abs_s = minute as u64 * 60 + dt.seconds as u64;
                    let _interval_s = last_log_abs_s.map_or(0, |a| abs_s.saturating_sub(a)).min(6 * 3600);
                    // "On battery" = no USB cable (the charging flag alone isn't reliable).
                    let ext = on_usb;
                    let due = match last_log_min {
                        None => true,
                        Some(m) => ext != last_log_ext || minute >= m + 10 || minute < m,
                    };
                    if due {
                        let mut flags = 0u8;
                        if charging { flags |= battlog::F_CHARGING; }
                        if on_usb { flags |= battlog::F_USB; }
                        let boot = last_log_min.is_none();
                        if boot { flags |= battlog::F_BOOT; }
                        let e = Entry {
                            seq: 0, minute, mv: batt_mv, pct: batt_pct, flags,
                            // Boot entries: screen_s holds the power chip's on/off source registers.
                            screen_s: if boot { ((pmu_on as u16) << 8) | pmu_off as u16 } else { (log_screen_ms / 1000).min(u16::MAX as u32) as u16 },
                            // Boot entries: the radio byte holds the reset reason instead.
                            radio_s: if boot { reset_code as u16 * 4 } else { (log_radio_ms / 1000).min(u16::MAX as u32) as u16 },
                            has_diag: true,
                            // Boot entries: the diag fields carry why/how we restarted:
                            // sleep_s = flags (1 super-watchdog, 2 brown-out now, 4 RTC memory kept),
                            // awake_s/timer/gpio = the last 4 loop steps before the restart.
                            sleep_s: if boot { (swd_flag as u16) | (((bod >> 31) as u16) << 1) | ((rtc_mem_kept as u16) << 2) }
                                     else { (log_sleep_ms / 1000).min(u16::MAX as u64) as u16 },
                            awake_s: if boot { ((trail[14] as u16) << 8) | trail[15] as u16 } else { (log_awake_ms / 1000).min(u16::MAX as u32) as u16 },
                            timer_wakes: if boot { trail[13] } else { log_timer_wakes },
                            // low nibble: touch/BOOT wakes (max 15); high nibble: brown-out events (max 15)
                            gpio_wakes: if boot { trail[12] } else { (log_gpio_wakes.min(15)) | (log_brownouts.min(15) << 4) },
                        };
                        crumb(42);
                        if !bat_log.append(e) { println!("battlog: append failed"); }
                        crumb(43);
                        log_saves = log_saves.wrapping_add(1);
                        last_log_min = Some(minute);
                        last_log_abs_s = Some(abs_s);
                        last_log_ext = ext;
                        log_screen_ms = 0;
                        log_radio_ms = 0;
                        log_awake_ms = 0;
                        log_timer_wakes = 0;
                        log_gpio_wakes = 0;
                        log_brownouts = 0;
                        log_sleep_ms = 0;
                    }
                }
            }
            next_battery = if screen_state == 0 {
                now + Duration::from_secs(600)
            } else {
                // Battery percent rarely changes faster than every few
                // minutes; polling every 60 s was pure I²C overhead.
                now + Duration::from_secs(180)
            };
        }

        crumb(50);
        // === Touch ===
        // Poll the I2C touch controller when:
        //   1. screen is on AND a finger is currently on the panel (INT low), OR
        //   2. screen is on AND the finger was on the panel last iteration (catches the
        //      lift/swipe-end event — without this, page swipes stay stuck mid-drag).
        // This keeps the bus quiet >99% of the time but never misses release events.
        let mut swipe_event = None;
        let mut finger_y: Option<u16> = None; // finger position while touching (launcher drag)
        let mut tap_event = false;
        let int_low = touch_int.is_low();
        // Touch I2C is only polled in fully-interactive states (AOD has no touch handling).
        // While the launcher list is being dragged/gliding, poll every frame so a
        // pulsed INT line can't make us miss finger positions (or the lift).
        let touch_active = screen_state >= 2
            && (int_low || was_touching || (app_state == AppState::Launcher && launcher.is_animating()));
        was_touching = int_low;
        if touch_active {
            if let Ok((point, event)) = touch.poll() {
            // Swipe handling for page navigation (only in Watchface mode)
            if app_state == AppState::Watchface {
                if let Some(tp) = point {
                    last_touch_x = tp.x;
                    last_touch_y = tp.y;
                    // Don't start a page swipe if the finger is on the
                    // brightness slider — horizontal drag there adjusts
                    // brightness, not pages.
                    let on_slider = current_page == Page::Clock
                        && WatchFace::brightness_from_tap(tp.x, tp.y).is_some();
                    if !swiping && !on_slider {
                        if swipe_start_x == 0 { swipe_start_x = tp.x as i32; }
                        else {
                            let dx = tp.x as i32 - swipe_start_x;
                            if dx.unsigned_abs() > 30 {
                                swiping = true;
                                swipe_dir = if dx < 0 { -1 } else { 1 };
                                snap_current.copy_from_slice(fb.buffer());
                                let target = if swipe_dir < 0 { current_page.next() } else { current_page.prev() };
                                fb.clear_color(target.color());
                                match target {
                                    Page::Clock => {
                                        let mut wf2 = WatchFace::new();
                                        if let Ok(dt) = rtc.get_time() { wf2.update_time(dt.hours, dt.minutes, dt.seconds); }
                                        wf2.update_battery(batt_pct, batt_mv, charging);
                                        wf2.wifi_connected = wifi_connected;
                                        wf2.force_redraw();
                                        let _ = wf2.render(&mut fb);
                                    }
                                    Page::Sensors => { let _ = pages::draw_sensors_page(&mut fb, 0,0,0,0,0,0,0); }
                                    Page::BigClock => {
                                        crate::ui::bigclock::draw_big_clock(&mut fb, clk.0, clk.1, clk.3, clk.4, clk.5, batt_pct, charging);
                                    }
                                    Page::System => { let _ = pages::draw_system_page(&mut fb, batt_mv, batt_pct, charging); }
                                    Page::Power => {
                                        update_power_stats(&mut power_stats, screen_state, imu_powered,
                                            wifi_connected, wifi_on_request, watchface.brightness,
                                            batt_mv, batt_pct, charging);
                                        let _ = power_page::draw_power_page(&mut fb, &power_stats);
                                    }
                                }
                                snap_target.copy_from_slice(fb.buffer());
                            }
                        }
                    }
                    if swiping {
                        let delta = (tp.x as i32 - swipe_start_x).clamp(-(board::LCD_WIDTH as i32), board::LCD_WIDTH as i32);
                        let offset = ((delta * swipe_dir).clamp(0, board::LCD_WIDTH as i32) as usize) & !1;
                        let w = board::LCD_WIDTH as usize;
                        let h = board::LCD_HEIGHT as usize;
                        if offset > 0 && offset < w {
                            if swipe_dir < 0 {
                                display.set_addr_window(0, 0, (w-offset) as u16, h as u16);
                                display.bus_mut().begin_pixels();
                                for row in 0..h { display.bus_mut().stream_pixels(&snap_current[row*w+offset..row*w+w]); }
                                display.bus_mut().end_pixels();
                                display.set_addr_window((w-offset) as u16, 0, offset as u16, h as u16);
                                display.bus_mut().begin_pixels();
                                for row in 0..h { display.bus_mut().stream_pixels(&snap_target[row*w..row*w+offset]); }
                                display.bus_mut().end_pixels();
                            } else {
                                display.set_addr_window(0, 0, offset as u16, h as u16);
                                display.bus_mut().begin_pixels();
                                for row in 0..h { display.bus_mut().stream_pixels(&snap_target[row*w+w-offset..row*w+w]); }
                                display.bus_mut().end_pixels();
                                display.set_addr_window(offset as u16, 0, (w-offset) as u16, h as u16);
                                display.bus_mut().begin_pixels();
                                for row in 0..h { display.bus_mut().stream_pixels(&snap_current[row*w..row*w+w-offset]); }
                                display.bus_mut().end_pixels();
                            }
                        }
                    }
                }
                if let Some(swipe) = event {
                    swipe_start_x = 0;
                    if swiping {
                        swiping = false;
                        let ok = matches!(
                            (&swipe.direction, swipe_dir),
                            (SwipeDirection::Left, -1) | (SwipeDirection::Right, 1)
                        );
                        if ok {
                            if swipe_dir < 0 { current_page = current_page.next(); }
                            else { current_page = current_page.prev(); }
                            fb.buffer_mut().copy_from_slice(&snap_target);
                            page_dirty = true;
                        } else {
                            fb.buffer_mut().copy_from_slice(&snap_current);
                            fb.flush(&mut display);
                        }
                    } else {
                        swipe_event = Some(swipe.direction);
                        tap_event = swipe.direction == SwipeDirection::Tap;
                    }
                }
            } else {
                // In app mode: track position + forward events
                if let Some(tp) = point {
                    last_touch_x = tp.x;
                    last_touch_y = tp.y;
                    finger_y = Some(tp.y);
                }
                if let Some(swipe) = event {
                    swipe_event = Some(swipe.direction);
                    tap_event = swipe.direction == SwipeDirection::Tap;
                }
            }
            }
        }

        // === Screen sleep/wake state machine ===
        // Levels:
        //   3 = full bright + interactive (default)
        //   2 = dim brightness, still interactive (transition state at 20 s idle)
        //   1 = AOD: minimal HH:MM, super-dim, 1 update/min, no I/O
        //   0 = full off (DISPOFF + SLPIN), only GPIO interrupts can wake
        //
        // Transitions on idle: 3 → (20s) → 2 → (40s) → 1 (AOD) → (10min) → 0 (off)
        // Any touch/button bumps us straight back to 3.
        let any_touch = touch_int.is_low() || woke_by_user;
        woke_by_user = false;
        if any_touch || swipe_event.is_some() || tap_event || boot_button.is_low() {
            last_interaction = now;
            if screen_state < 3 {
                // Wake up to full bright. If we were fully off (state 0), re-init the panel.
                if screen_state == 0 {
                    // Back to the user's CPU speed before driving the panel.
                    crate::peripherals::cpu_clock::set_cpu_mhz(watchface.cpu_mhz);
                    crumb(80); display.display_on();
                    Timer::after(Duration::from_millis(20)).await;
                }
                // Restore the user's chosen brightness from the slider.
                display.set_brightness(watchface.brightness);
                screen_state = 3;
                next_watchface_flush = now;
                if app_state == AppState::Watchface {
                    watchface.force_redraw();
                    page_dirty = true;
                }
            }
        }
        if app_state == AppState::Flashlight {
            if tap_event { flash_last_tap = now; }
            if (now - flash_last_tap).as_secs() < flashlight::KEEP_AWAKE_SECS {
                last_interaction = now;
            }
        }
        let idle_secs = (now - last_interaction).as_secs();
        // 3 min in AOD → fully off (was 10 min — aggressive saves ~8 mA×7 min)
        if idle_secs >= 180 && screen_state > 0 {
            display.set_brightness(0x00);
            crumb(81); display.display_off();
            screen_state = 0;
            if !wifi_started { crate::peripherals::cpu_clock::set_cpu_mhz(80); }
        // 15 s idle → AOD (was 40 s — faster dim saves ~45 mA×25 s every cycle)
        } else if idle_secs >= 15 && screen_state > 1 {
            if app_state == AppState::Watchface && (current_page == Page::Clock || current_page == Page::BigClock) {
                display.set_brightness(0x18); // very dim, ~10% of normal
                screen_state = 1;
                aod_last_minute = 99; // force first AOD frame
            } else {
                // Not on the clock face → no AOD, just go straight to off
                display.set_brightness(0x00);
                crumb(81); display.display_off();
                screen_state = 0;
                if !wifi_started { crate::peripherals::cpu_clock::set_cpu_mhz(80); }
            }
        // 8 s idle → dim transition (was 20 s)
        } else if idle_secs >= 8 && screen_state > 2 {
            display.set_brightness(0x40);
            screen_state = 2;
        }

        crumb(60);
        // === WiFi on/off state machine ===
        //
        // We take exactly one action per loop iteration so we don't block
        // the UI during a long-running connect_async(). User wants (wifi_on_request)
        // is driven by tapping the 'W' button on the watchface.
        //
        // On enable:  start() -> connect_async() -> set_power_save(MaxModem) -> NTP (once)
        // On disable: disconnect_async() -> stop()
        //
        // An "auto-off after 5 minutes idle" safety net is kept so that if
        // the user leaves WiFi enabled and wanders off, the radio drops on
        // its own. Turning it back on is manual — intentional.
        // Debounce the WiFi button: ignore rapid re-taps within 1 s.
        if wifi_toggle_request && (now - last_wifi_idle_check).as_millis() >= 1000 {
            wifi_on_request = !wifi_on_request;
            wifi_toggle_request = false;
            last_wifi_idle_check = now;
            println!("[WIFI] User toggled → {}", if wifi_on_request { "ON" } else { "OFF" });
        } else if wifi_toggle_request {
            wifi_toggle_request = false; // swallow the bounce
        }

        if wifi_on_request && !wifi_connected {
            crumb(61);
            // Try each allowed network in order (Auto: home, then hotspot),
            // 8 s each, so a missing AP / wrong password never blocks for long.
            let mut ok = false;
            for &net in settings_app.candidates() {
                let Some(cfg) = wifi_net_config(net) else { continue };
                if wifi_cfg_net != net {
                    // Switch networks with the radio stopped.
                    if wifi_started { let _ = wifi_controller.stop(); wifi_started = false; }
                    if wifi_controller.set_config(&cfg).is_err() { continue; }
                    wifi_cfg_net = net;
                }
                if !wifi_started {
                    if wifi_controller.start().is_ok() { wifi_started = true; } else { continue; }
                }
                println!("[WIFI] Trying network {}", net);
                if let Ok(Ok(())) = embassy_time::with_timeout(
                    Duration::from_secs(8),
                    wifi_controller.connect_async(),
                ).await {
                    ok = true;
                    wifi_net = net;
                    break;
                }
            }
            {
                match ok {
                    true => {
                        println!("[WIFI] Connected (PS=MaxModem)");
                        wifi_connected = true;
                        wifi_failed = false;
                        watchface.wifi_connected = true;
                        watchface.force_redraw();
                        page_dirty = true;
                        // NTP runs from the retry block below, once DHCP lands.
                    }
                    false => {
                        wifi_failed = true;
                        // Timeout or error — back off instead of hammering.
                        // POWER FIX: also stop the radio. Previously it was left
                        // started (scanning/retrying at ~80-100 mA) until the
                        // battery died, because stop() only ran after a
                        // successful connection.
                        println!("[WIFI] Connect failed/timeout -> radio off");
                        let _ = wifi_controller.stop();
                        wifi_started = false;
                        wifi_on_request = false;
                        watchface.wifi_connected = false;
                        watchface.force_redraw();
                        page_dirty = true;
                    }
                }
            }
            last_wifi_idle_check = now;
        }

        // NTP: retry every 10 s while connected until one sync succeeds.
        // The old code waited only 3 s for DHCP right after connecting and,
        // if the lease wasn't there yet, silently skipped NTP for the whole boot.
        if wifi_connected && !ntp_synced
            && last_ntp_attempt.map_or(true, |t| (now - t).as_millis() >= 10_000)
        {
            last_ntp_attempt = Some(now);
            if stack.config_v4().is_none() {
                println!("[NTP] waiting for DHCP lease");
            } else if ntp_sync(stack, &mut rtc).await.is_ok() {
                ntp_synced = true;
                println!("[NTP] synced");
            } else {
                println!("[NTP] sync failed, retrying in 10 s");
            }
        }

        // Settings: network choice changed while connected -> drop and reconnect
        // (wifi_on_request stays on, so the connect block above runs again).
        if settings_app.take_reconnect() && wifi_connected {
            let _ = wifi_controller.disconnect_async().await;
            let _ = wifi_controller.stop();
            wifi_started = false;
            wifi_connected = false;
            wifi_net = 255;
            println!("[WIFI] Reconnecting with new network choice");
        }
        if !wifi_on_request && wifi_connected {
            let _ = wifi_controller.disconnect_async().await;
            println!("[WIFI] Disconnected");
            wifi_connected = false;
            wifi_net = 255;
            watchface.wifi_connected = false;
            let _ = wifi_controller.stop();
            wifi_started = false;
            watchface.force_redraw();
            page_dirty = true;
            last_wifi_idle_check = now;
        }
        // Belt and braces: radio started but not wanted and not connected → stop it.
        if !wifi_on_request && wifi_started && !wifi_connected {
            let _ = wifi_controller.stop();
            wifi_started = false;
            println!("[WIFI] Radio stopped (not wanted)");
        }
        // Safety net: WiFi left on, user idle 5 min → auto-off.
        if wifi_on_request && idle_secs >= 300
            && (now - last_wifi_idle_check).as_secs() >= 60 {
            wifi_on_request = false;
            last_wifi_idle_check = now;
        }

        // === BLE state machine ===
        if ble_toggle_request {
            ble_toggle_request = false;
            ble_on = !ble_on;
            if ble_on && ble_connector.is_none() {
                if let Some(bt) = bt_periph.take() {
                    match esp_radio::ble::controller::BleConnector::new(
                        radio_controller, bt, esp_radio::ble::Config::default(),
                    ) {
                        Ok(c) => ble_connector = Some(c),
                        Err(_) => println!("[BLE] init failed"),
                    }
                }
            }
            if ble_on && ble_connector.is_none() {
                ble_on = false;
            } else if ble_on {
                match crate::peripherals::ble::start_advertising(ble_connector.as_mut().unwrap()) {
                    Ok(()) => println!("[BLE] Advertising started"),
                    Err(_) => {
                        println!("[BLE] Failed to start advertising");
                        ble_on = false;
                    }
                }
            } else {
                if let Some(c) = ble_connector.as_mut() {
                    let _ = crate::peripherals::ble::stop_advertising(c);
                }
                println!("[BLE] Advertising stopped");
            }
            watchface.ble_on = ble_on;
            power_stats.ble_on = ble_on;
            watchface.force_redraw();
            page_dirty = true;
        }

        // === Countdown timer alarm ===
        // Runs before the AOD / screen-off early-outs so it fires even while asleep:
        // wake the screen, jump to the Timer app, and beep.
        if timer_app.check_expired() {
            if screen_state == 0 {
                crate::peripherals::cpu_clock::set_cpu_mhz(watchface.cpu_mhz);
                crumb(80); display.display_on();
                Timer::after(Duration::from_millis(20)).await;
            }
            display.set_brightness(watchface.brightness);
            screen_state = 3;
            last_interaction = Instant::now();
            app_state = AppState::Timer;
            timer_app.render(&mut fb);
            fb.flush(&mut display);
            // 3 groups of 3 short beeps.
            for group in 0..3 {
                beep_burst!(3);
                if group < 2 { Timer::after(Duration::from_millis(400)).await; }
            }
            next_watchface_flush = Instant::now();
            continue;
        }

        // === Daily alarm ===
        // Checked about once a second while awake, and on every wake from AOD /
        // light sleep (sleep is timed to end right at the alarm minute).
        if alarm_app.is_armed() && now >= next_alarm_check {
            next_alarm_check = now + Duration::from_secs(1);
            if let Ok(dt) = rtc.get_time() {
                clk = (dt.hours, dt.minutes, dt.seconds, dt.day, dt.month, dt.year);
                if alarm_app.check(dt.hours, dt.minutes) {
                    if screen_state == 0 {
                        crate::peripherals::cpu_clock::set_cpu_mhz(watchface.cpu_mhz);
                        crumb(80); display.display_on();
                        Timer::after(Duration::from_millis(20)).await;
                    }
                    display.set_brightness(watchface.brightness);
                    screen_state = 3;
                    last_interaction = Instant::now();
                    app_state = AppState::Alarm;
                    alarm_app.render(&mut fb);
                    fb.flush(&mut display);
                    alarm_ring_start = Instant::now();
                    alarm_next_beep = Instant::now();
                    next_watchface_flush = Instant::now();
                    continue;
                }
            }
        }

        // === AOD render path ===
        // In AOD we render *only* when the minute changes. Reads RTC, draws minimal
        // black-background HH:MM into the framebuffer, flushes once. Total work per minute:
        // ~1 RTC read + ~1 framebuffer fill + 1 DMA flush. The CPU sleeps the rest of the time.
        if screen_state == 1 {
            if let Ok(dt) = rtc.get_time() {
                if dt.minutes != aod_last_minute {
                    aod_last_minute = dt.minutes;
                    watchface.update_time(dt.hours, dt.minutes, dt.seconds);
                    if let Ok(pct) = power.get_battery_percent() {
                        watchface.update_battery(pct, batt_mv, charging);
                    }
                    let _ = watchface.render_aod(&mut fb);
                    fb.flush(&mut display);
                }
            }
            continue; // skip the normal app/render path
        }

        // When the screen is off, skip all rendering/flushing.
        // Keeps the QSPI bus idle so the CO5300 stays in a clean sleep state,
        // and the wake-up set_brightness/display_on commands always get through.
        if screen_state == 0 {
            continue;
        }

        crumb(70);
        // === App state machine ===
        // Any time we're not in the launcher, make sure it draws once when opened.
        if app_state != AppState::Launcher { launcher_needs_draw = true; }
        match app_state {
            AppState::Watchface => {
                if !swiping {
                    let mut need_flush = false;
                    if page_dirty {
                        fb.clear_color(current_page.color());
                        match current_page {
                            Page::Clock => { watchface.force_redraw(); }
                            Page::BigClock => { bigclock_drawn = 0xFFFF; }
                            Page::System => { let _ = pages::draw_system_page(&mut fb, batt_mv, batt_pct, charging); }
                            Page::Power => {
                                // Rebuild stats snapshot, then render once.
                                // Subsequent frames will only redraw every
                                // ~1 s (see below) to keep the diagnostic
                                // itself cheap.
                                update_power_stats(&mut power_stats, screen_state, imu_powered,
                                    wifi_connected, wifi_on_request, watchface.brightness,
                                    batt_mv, batt_pct, charging);
                                let _ = power_page::draw_power_page(&mut fb, &power_stats);
                            }
                            _ => {}
                        }
                        page_dirty = false;
                        need_flush = true;
                    }
                    match current_page {
                        Page::Clock => {
                            // Only render if WatchFace says something is dirty.
                            if watchface.needs_render() {
                                let _ = watchface.render(&mut fb);
                                need_flush = true;
                            }
                        }
                        Page::Sensors => {
                            // Sensors page is repainted at the loop tick rate (10 Hz).
                            let ax = (accel.0 * 100.0) as i16;
                            let ay = (accel.1 * 100.0) as i16;
                            let az = (accel.2 * 100.0) as i16;
                            fb.clear_color(current_page.color());
                            let _ = pages::draw_sensors_page(&mut fb, ax, ay, az, gyro_data.0, gyro_data.1, gyro_data.2, imu_temp);
                            need_flush = true;
                        }
                        Page::Power => {
                            // Refresh the snapshot + redraw at ~1 Hz. Any faster
                            // and the diagnostic itself starts to skew the
                            // measurement it's supposed to report.
                            if now >= next_watchface_flush {
                                update_power_stats(&mut power_stats, screen_state, imu_powered,
                                    wifi_connected, wifi_on_request, watchface.brightness,
                                    batt_mv, batt_pct, charging);
                                let _ = power_page::draw_power_page(&mut fb, &power_stats);
                                need_flush = true;
                                next_watchface_flush = now + Duration::from_secs(1);
                            }
                        }
                        Page::System => {} // Static, already rendered
                        Page::BigClock => {
                            let mod_min = clk.0 as u16 * 60 + clk.1 as u16;
                            if mod_min != bigclock_drawn {
                                crate::ui::bigclock::draw_big_clock(&mut fb, clk.0, clk.1, clk.3, clk.4, clk.5, batt_pct, charging);
                                bigclock_drawn = mod_min;
                                need_flush = true;
                            }
                        }
                    }
                    // Only flush if we actually drew something. The TE wait + 402 KB DMA
                    // is by far the heaviest periodic operation in the firmware, so we
                    // gate it strictly on dirtiness.
                    if need_flush {
                        fb.flush_vsync(&mut display, &te_pin);
                        next_watchface_flush = now;
                    }
                }

                // Tap/touch dispatch on the Clock page.
                if current_page == Page::Clock {
                    // Brightness slider — responds to both taps and held
                    // drags so you can slide your finger along it.
                    if let Some(bri) = WatchFace::brightness_from_tap(last_touch_x, last_touch_y) {
                        if (touch_int.is_low() || tap_event) && bri != watchface.brightness {
                            watchface.brightness = bri;
                            display.set_brightness(bri);
                            watchface.force_redraw();
                            page_dirty = true;
                        }
                    } else if tap_event {
                        // BLE toggle
                        if WatchFace::is_ble_zone(last_touch_x, last_touch_y) {
                            ble_toggle_request = true;
                            watchface.force_redraw();
                            page_dirty = true;
                        // WiFi toggle
                        } else if WatchFace::is_wifi_zone(last_touch_x, last_touch_y) {
                            wifi_toggle_request = true;
                            watchface.force_redraw();
                            page_dirty = true;
                        // CPU frequency cycle (live DVFS)
                        } else if WatchFace::is_cpu_zone(last_touch_x, last_touch_y) {
                            watchface.cycle_cpu();
                            let actual = crate::peripherals::cpu_clock::set_cpu_mhz(watchface.cpu_mhz);
                            watchface.cpu_mhz = actual;
                            power_stats.cpu_mhz = actual;
                            println!("CPU freq: {}MHz (live)", actual);
                            watchface.force_redraw();
                            page_dirty = true;
                        // Apps launcher
                        } else if WatchFace::is_apps_zone(last_touch_x, last_touch_y) {
                            app_state = AppState::Launcher;
                        // Gyro toggle
                        } else if WatchFace::is_gyro_zone(last_touch_y) {
                            let enabled = watchface.toggle_gyro();
                            println!("Gyro: {}", if enabled { "ON" } else { "OFF" });
                        }
                    }
                }

                // Reboot button on Power page
                if current_page == Page::Power && tap_event {
                    if power_page::is_reboot_zone(last_touch_x, last_touch_y) {
                        println!("REBOOT requested");
                        esp_hal::system::software_reset();
                    }
                }

                // Swipe up on Clock → launcher, boot button too
                if let Some(SwipeDirection::Up) = swipe_event {
                    if current_page == Page::Clock || current_page == Page::BigClock {
                        app_state = AppState::Launcher;
                    }
                }
                if boot_button.is_low() {
                    app_state = AppState::Launcher;
                    Timer::after(Duration::from_millis(200)).await;
                }
            }

            AppState::Launcher => {
                // Finger position comes from the main touch poll above. (A second
                // poll here used to swallow release events, losing swipes.)
                if let Some(new_state) = launcher.update(finger_y, swipe_event, tap_event, last_touch_y) {
                    app_state = new_state;
                    match app_state {
                        AppState::Tetris => tetris_game.setup(),
                        AppState::Maze => maze_game.setup(),
                        AppState::SmartHome => smarthome_app.setup(),
                        AppState::Stopwatch => { stopwatch_app.setup(); next_watchface_flush = now; }
                        AppState::Timer => { timer_app.setup(); next_watchface_flush = now; }
                        AppState::Flashlight => {
                            flashlight_app.setup();
                            flash_last_tap = now;
                            display.set_brightness(flashlight_app.brightness());
                            next_watchface_flush = now;
                        }
                        AppState::Steps => { steps_app.setup(); steps_dirty = true; next_watchface_flush = now; }
                        AppState::HaStatus => { ha_status_app.setup(); next_watchface_flush = now; }
                        AppState::Weather => { weather_app.setup(); next_watchface_flush = now; }
                        AppState::Fleet => { fleet_app.setup(); next_watchface_flush = now; }
                        AppState::Alarm => { alarm_app.setup(); next_watchface_flush = now; }
                        AppState::Calculator => { calc_app.setup(); next_watchface_flush = now; }
                        AppState::Level => { level_app.setup(); next_watchface_flush = now; }
                        AppState::Dice => { dice_app.setup(); next_watchface_flush = now; }
                        AppState::SunMoon => {
                            if let Ok(dt) = rtc.get_time() {
                                sunmoon_app.set_date(2000 + dt.year as i32, dt.month as u32, dt.day as u32, dt.hours as u32);
                            }
                            next_watchface_flush = now;
                        }
                        AppState::TicTacToe => { ttt_app.setup(); next_watchface_flush = now; }
                        AppState::Asteroids => { asteroids_app.setup(); }
                        AppState::Waves => {
                            waves_app.setup();
                            crate::peripherals::cpu_clock::set_cpu_mhz(240);
                        }
                        AppState::Fluid => {
                            fluid_app.setup();
                            // Full speed while the simulation runs; restored on exit.
                            crate::peripherals::cpu_clock::set_cpu_mhz(240);
                        }
                        AppState::WifiQr => { wifiqr_app.setup(); next_watchface_flush = now; }
                        AppState::Battery => {
                            battery_app.load(bat_log.read_all(), bat_log.ok);
                            battery_app.clear_status();
                            battery_app_saves = log_saves;
                            next_battery_live = now; // live reading on the first frame
                            next_watchface_flush = now;
                        }
                        AppState::Settings => { next_watchface_flush = now; } // draw it right away
                        AppState::Watchface => { watchface.force_redraw(); page_dirty = true; }
                        _ => {}
                    }
                } else if launcher_needs_draw || launcher.is_animating() || finger_y.is_some() {
                    // Redraw only while the list moves (plus once on entry).
                    launcher.render(&mut fb);
                    fb.flush(&mut display);
                    launcher_needs_draw = false;
                }
                if boot_button.is_low() {
                    app_state = AppState::Watchface;
                    watchface.force_redraw();
                    page_dirty = true;
                    Timer::after(Duration::from_millis(200)).await;
                }
            }

            AppState::Tetris => {
                let input = AppInput { touch: None, swipe: swipe_event, tap: tap_event, accel, dt_ms: dt_ms.max(1) };
                tetris_game.update(&input);
                if tetris_game.stepped() || swipe_event.is_some() || tap_event {
                    tetris_game.render(&mut fb);
                    fb.flush_vsync(&mut display, &te_pin);
                }
                if boot_button.is_low() { app_state = AppState::Launcher; Timer::after(Duration::from_millis(200)).await; }
            }

            AppState::Maze => {
                let input = AppInput { touch: None, swipe: swipe_event, tap: tap_event, accel, dt_ms: dt_ms.max(1) };
                maze_game.update(&input);
                // Maze renders at 30fps (IMU continuous)
                if now >= next_watchface_flush {
                    maze_game.render(&mut fb);
                    fb.flush_vsync(&mut display, &te_pin);
                    next_watchface_flush = now + Duration::from_millis(33);
                }
                if boot_button.is_low() { app_state = AppState::Launcher; Timer::after(Duration::from_millis(200)).await; }
            }

            AppState::SmartHome => {
                let input = AppInput { touch: None, swipe: swipe_event, tap: false, accel, dt_ms: dt_ms.max(1) };
                smarthome_app.update(&input);
                // Hit-test the tap against the button list (update() only scrolls).
                if tap_event {
                    smarthome_app.handle_tap(last_touch_y);
                }

                // Fire the HTTP request for a tapped button. The URL is copied out of
                // the app first, so the borrow ends before we store the response.
                if let Some((idx, url_buf, url_len, method)) = smarthome_app.take_pending() {
                    // Repaint right away so the button shows "..." while we wait.
                    smarthome_app.render(&mut fb);
                    fb.flush_vsync(&mut display, &te_pin);

                    let url = core::str::from_utf8(&url_buf[..url_len]).unwrap_or("");
                    let result = if !wifi_connected || stack.config_v4().is_none() {
                        Err(())
                    } else {
                        match method {
                            HttpMethod::Get => http::http_get(stack, url).await,
                            HttpMethod::Post => http::http_post(stack, url, "{}").await,
                        }
                    };
                    match result {
                        Ok(r) if (200..300).contains(&r.status) => smarthome_app.set_response(idx, "OK", true),
                        Ok(_) => smarthome_app.set_response(idx, "ERR", false),
                        Err(_) => smarthome_app.set_response(idx, "NET", false),
                    }
                    next_watchface_flush = Instant::now();
                }

                smarthome_app.render(&mut fb);
                if now >= next_watchface_flush {
                    fb.flush_vsync(&mut display, &te_pin);
                    next_watchface_flush = now + Duration::from_millis(100);
                }
                if boot_button.is_low() { app_state = AppState::Launcher; Timer::after(Duration::from_millis(200)).await; }
            }

            AppState::Stopwatch => {
                let changed = tap_event && stopwatch_app.handle_tap(last_touch_x, last_touch_y);
                if changed || stopwatch_app.is_running() || now >= next_watchface_flush {
                    stopwatch_app.render(&mut fb);
                    fb.flush_vsync(&mut display, &te_pin);
                    next_watchface_flush = now + Duration::from_secs(1);
                }
                if boot_button.is_low() { app_state = AppState::Launcher; Timer::after(Duration::from_millis(200)).await; }
            }

            AppState::Timer => {
                let changed = tap_event && timer_app.handle_tap(last_touch_x, last_touch_y);
                if changed || timer_app.is_running() || now >= next_watchface_flush {
                    timer_app.render(&mut fb);
                    fb.flush_vsync(&mut display, &te_pin);
                    next_watchface_flush = now + Duration::from_secs(1);
                }
                if boot_button.is_low() { app_state = AppState::Launcher; Timer::after(Duration::from_millis(200)).await; }
            }

            AppState::Flashlight => {
                if tap_event { flashlight_app.handle_tap(); }
                // Re-apply the flashlight brightness whenever we're fully awake
                // (waking from sleep restores the watchface brightness).
                if screen_state == 3 {
                    display.set_brightness(flashlight_app.brightness());
                }
                if tap_event || now >= next_watchface_flush {
                    flashlight_app.render(&mut fb);
                    fb.flush_vsync(&mut display, &te_pin);
                    next_watchface_flush = now + Duration::from_secs(5);
                }
                if boot_button.is_low() {
                    display.set_brightness(watchface.brightness);
                    app_state = AppState::Launcher;
                    Timer::after(Duration::from_millis(200)).await;
                }
            }

            AppState::Steps => {
                if tap_event && steps_app.handle_tap(last_touch_x, last_touch_y) {
                    steps_dirty = true;
                }
                if steps_dirty || now >= next_watchface_flush {
                    steps_app.render(&mut fb);
                    fb.flush_vsync(&mut display, &te_pin);
                    steps_dirty = false;
                    next_watchface_flush = now + Duration::from_secs(2);
                }
                if boot_button.is_low() {
                    steps_app.stop(); // leaving the app powers the sensor down
                    app_state = AppState::Launcher;
                    Timer::after(Duration::from_millis(200)).await;
                }
            }

            AppState::HaStatus | AppState::Weather | AppState::Fleet => {
                let app = match app_state {
                    AppState::HaStatus => &mut ha_status_app,
                    AppState::Weather => &mut weather_app,
                    _ => &mut fleet_app,
                };
                if tap_event {
                    app.handle_tap();
                    next_watchface_flush = now;
                }
                if app.needs_fetch() {
                    if let Some(token) = ha_info::HA_TOKEN {
                        let net_ready = wifi_connected && stack.config_v4().is_some();
                        if net_ready {
                            app.set_loading();
                            app.render(&mut fb);
                            fb.flush_vsync(&mut display, &te_pin);
                            let tpl = app.template();
                            let res = embassy_time::with_timeout(
                                Duration::from_secs(15),
                                http::ha_template(stack, ha_info::HA_HOST, token, &tpl),
                            ).await;
                            let (hh, mm) = rtc.get_time().map(|d| (d.hours, d.minutes)).unwrap_or((0, 0));
                            match res {
                                Ok(Ok((status, body))) => app.set_result(status, &body, hh, mm),
                                Ok(Err(_)) => app.set_error("Can't reach HA"),
                                Err(_) => app.set_error("HA timed out"),
                            }
                            if ha_wifi_auto {
                                wifi_on_request = false; // radio off again right away
                                ha_wifi_auto = false;
                            }
                            next_watchface_flush = Instant::now();
                        } else if !app.is_waiting_wifi() {
                            // First attempt: bring Wi-Fi up if it's off.
                            if !wifi_on_request {
                                wifi_on_request = true;
                                ha_wifi_auto = true;
                            }
                            ha_wait_since = now;
                            app.set_waiting_wifi();
                            next_watchface_flush = now;
                        } else if !wifi_on_request {
                            // The Wi-Fi state machine gave up (bad creds / no AP).
                            app.set_error("Wi-Fi connect failed");
                            ha_wifi_auto = false;
                            next_watchface_flush = now;
                        } else if (now - ha_wait_since).as_secs() >= 25 {
                            app.set_error("No network (DHCP)");
                            if ha_wifi_auto { wifi_on_request = false; ha_wifi_auto = false; }
                            next_watchface_flush = now;
                        }
                    }
                }
                if now >= next_watchface_flush {
                    app.render(&mut fb);
                    fb.flush_vsync(&mut display, &te_pin);
                    next_watchface_flush = now + Duration::from_secs(if app.needs_fetch() { 1 } else { 30 });
                }
                if boot_button.is_low() {
                    if ha_wifi_auto { wifi_on_request = false; ha_wifi_auto = false; }
                    app_state = AppState::Launcher;
                    Timer::after(Duration::from_millis(200)).await;
                }
            }

            AppState::Alarm => {
                let changed = tap_event && alarm_app.handle_tap(last_touch_x, last_touch_y, clk.0, clk.1);
                if alarm_app.is_ringing() {
                    last_interaction = now; // keep the screen on while ringing
                    if (now - alarm_ring_start).as_secs() >= alarm::RING_SECS {
                        alarm_app.stop();
                        next_watchface_flush = now;
                    } else if now >= alarm_next_beep {
                        beep_burst!(3);
                        alarm_next_beep = Instant::now() + Duration::from_millis(1200);
                    }
                }
                if changed || now >= next_watchface_flush {
                    alarm_app.render(&mut fb);
                    fb.flush_vsync(&mut display, &te_pin);
                    next_watchface_flush = now + Duration::from_secs(5);
                }
                if boot_button.is_low() {
                    alarm_app.stop();
                    app_state = AppState::Launcher;
                    Timer::after(Duration::from_millis(200)).await;
                }
            }

            AppState::Calculator | AppState::TicTacToe | AppState::SunMoon | AppState::WifiQr | AppState::Battery => {
                // Tap-driven / static screens: redraw on change only.
                let changed = tap_event && match app_state {
                    AppState::Calculator => calc_app.handle_tap(last_touch_x, last_touch_y),
                    AppState::TicTacToe => ttt_app.handle_tap(last_touch_x, last_touch_y),
                    AppState::Battery => battery_app.handle_tap(last_touch_x, last_touch_y),
                    _ => false,
                };
                if app_state == AppState::Battery {
                    // Reload when a new record was saved; refresh the live reading every 5 s.
                    let mut dirty = false;
                    if battery_app_saves != log_saves {
                        battery_app.load(bat_log.read_all(), bat_log.ok);
                        battery_app_saves = log_saves;
                        dirty = true;
                    }
                    if now >= next_battery_live {
                        let now_min = match rtc.get_time() {
                            Ok(dt) => battlog::rtc_minute(dt.year, dt.month, dt.day, dt.hours, dt.minutes),
                            Err(_) => 0,
                        };
                        let pct = power.get_battery_percent().unwrap_or(batt_pct);
                        let mv = power.get_battery_voltage().unwrap_or(batt_mv);
                        let chg = power.is_charging().unwrap_or(charging);
                        let usb = power.is_vbus_in().unwrap_or(usb_present);
                        battery_app.set_live(now_min, pct, mv, chg, usb);
                        next_battery_live = now + Duration::from_secs(5);
                        dirty = true;
                    }
                    if dirty { next_watchface_flush = now; }
                }
                match battery_app.take_export() {
                    crate::apps::battery::Export::Usb => {
                        // CSV over the USB serial console (any serial monitor; baud doesn't matter on USB).
                        print_battery_csv(battery_app.entries());
                        let mut m: crate::apps::widgets::SBuf<44> = crate::apps::widgets::SBuf::new();
                        let _ = core::fmt::Write::write_fmt(&mut m, format_args!("Sent {} records to USB serial", battery_app.entries().len()));
                        battery_app.set_status(m.as_str(), crate::apps::widgets::TEAL_LIGHT);
                        next_watchface_flush = now;
                    }
                    crate::apps::battery::Export::Wifi => {
                        if !wifi_on_request { wifi_on_request = true; bat_wifi_auto = true; }
                        bat_wifi_wait = Some(now);
                        battery_app.set_status("Connecting to Wi-Fi...", crate::apps::widgets::AMBER_LIGHT);
                        next_watchface_flush = now;
                    }
                    crate::apps::battery::Export::None => {}
                }
                if let Some(since) = bat_wifi_wait {
                    let net_ready = wifi_connected && stack.config_v4().is_some();
                    if net_ready {
                        bat_wifi_wait = None;
                        // Serve the log until BOOT is pressed or 3 minutes pass.
                        let ip = stack.config_v4().map(|c| c.address.address());
                        let mut m: crate::apps::widgets::SBuf<44> = crate::apps::widgets::SBuf::new();
                        if let Some(ip) = ip { let _ = core::fmt::Write::write_fmt(&mut m, format_args!("Open http://{}/ (tap=stop)", ip)); }
                        battery_app.set_status(m.as_str(), crate::apps::widgets::GREEN_LIGHT);
                        battery_app.render(&mut fb);
                        fb.flush_vsync(&mut display, &te_pin);
                        // Radio fully awake while serving: with modem power save the
                        // radio naps between beacons and phone hotspots don't buffer
                        // for it, so pages timed out / needed many refreshes.
                        let _ = wifi_controller.set_power_saving(esp_radio::wifi::PowerSaveMode::None);
                        let deadline = Instant::now() + Duration::from_secs(180);
                        let mut sent = 0u32;
                        loop {
                            // Ends on: time-out, BOOT, or any touch on the screen (so tapping
                            // USB or anything else while a Wi-Fi export runs isn't ignored).
                            let r = embassy_futures::select::select4(
                                crate::peripherals::webexport::serve_one(stack, battery_app.entries()),
                                Timer::at(deadline),
                                boot_button.wait_for_falling_edge(),
                                touch_int.wait_for_falling_edge(),
                            ).await;
                            last_interaction = Instant::now(); // keep the screen on meanwhile
                            match r {
                                embassy_futures::select::Either4::First(crate::peripherals::webexport::Served::Csv) => {
                                    sent += 1;
                                    let mut m2: crate::apps::widgets::SBuf<44> = crate::apps::widgets::SBuf::new();
                                    if let Some(ip) = ip { let _ = core::fmt::Write::write_fmt(&mut m2, format_args!("Sent! {} - tap to stop", ip)); }
                                    battery_app.set_status(m2.as_str(), crate::apps::widgets::GREEN_LIGHT);
                                    battery_app.render(&mut fb);
                                    fb.flush_vsync(&mut display, &te_pin);
                                }
                                embassy_futures::select::Either4::First(_) => {}
                                _ => break, // timeout, BOOT or a touch
                            }
                        }
                        let _ = wifi_controller.set_power_saving(esp_radio::wifi::PowerSaveMode::Maximum);
                        // Don't let the BOOT press that ended the export also leave the app.
                        while boot_button.is_low() { Timer::after(Duration::from_millis(20)).await; }
                        if bat_wifi_auto { wifi_on_request = false; bat_wifi_auto = false; }
                        let mut m3: crate::apps::widgets::SBuf<44> = crate::apps::widgets::SBuf::new();
                        let _ = core::fmt::Write::write_fmt(&mut m3, format_args!("Wi-Fi export done ({} sent)", sent));
                        battery_app.set_status(m3.as_str(), if sent > 0 { crate::apps::widgets::GREEN_LIGHT } else { crate::apps::widgets::AMBER_LIGHT });
                        last_interaction = Instant::now();
                        next_watchface_flush = Instant::now();
                        continue;
                    } else if !wifi_on_request {
                        bat_wifi_wait = None;
                        battery_app.set_status("Wi-Fi connect failed", crate::apps::widgets::CORAL_LIGHT);
                        bat_wifi_auto = false;
                        next_watchface_flush = now;
                    } else if (now - since).as_secs() >= 25 {
                        bat_wifi_wait = None;
                        battery_app.set_status("No network (timed out)", crate::apps::widgets::CORAL_LIGHT);
                        if bat_wifi_auto { wifi_on_request = false; bat_wifi_auto = false; }
                        next_watchface_flush = now;
                    }
                }
                if changed || now >= next_watchface_flush {
                    match app_state {
                        AppState::Calculator => calc_app.render(&mut fb),
                        AppState::TicTacToe => ttt_app.render(&mut fb),
                        AppState::SunMoon => sunmoon_app.render(&mut fb),
                        AppState::Battery => battery_app.render(&mut fb),
                        _ => wifiqr_app.render(&mut fb),
                    }
                    fb.flush_vsync(&mut display, &te_pin);
                    next_watchface_flush = now + Duration::from_secs(60);
                }
                if boot_button.is_low() {
                    if app_state == AppState::Battery {
                        bat_wifi_wait = None;
                        if bat_wifi_auto { wifi_on_request = false; bat_wifi_auto = false; }
                    }
                    app_state = AppState::Launcher;
                    Timer::after(Duration::from_millis(200)).await;
                }
            }

            AppState::Level | AppState::Dice => {
                let input = AppInput { touch: None, swipe: swipe_event, tap: tap_event, accel, dt_ms: dt_ms.max(1) };
                if app_state == AppState::Level {
                    level_app.update(&input);
                    level_app.render(&mut fb);
                    fb.flush_vsync(&mut display, &te_pin);
                } else {
                    if tap_event { dice_app.handle_tap(last_touch_x, last_touch_y); }
                    let was_rolling = dice_app.is_rolling();
                    dice_app.update(&input);
                    if was_rolling || dice_app.is_rolling() || tap_event || now >= next_watchface_flush {
                        dice_app.render(&mut fb);
                        fb.flush_vsync(&mut display, &te_pin);
                        next_watchface_flush = now + Duration::from_secs(5);
                    }
                }
                if boot_button.is_low() { app_state = AppState::Launcher; Timer::after(Duration::from_millis(200)).await; }
            }

            AppState::Asteroids => {
                let input = AppInput { touch: None, swipe: swipe_event, tap: tap_event, accel, dt_ms: dt_ms.max(1) };
                asteroids_app.update(&input);
                // Keep the screen awake while a game is running.
                if !asteroids_app.is_over() { last_interaction = now; }
                asteroids_app.render(&mut fb);
                fb.flush_vsync(&mut display, &te_pin);
                if boot_button.is_low() { app_state = AppState::Launcher; Timer::after(Duration::from_millis(200)).await; }
            }

            AppState::Fluid | AppState::Waves => {
                let input = AppInput { touch: None, swipe: swipe_event, tap: tap_event, accel, dt_ms: dt_ms.max(1) };
                // Moving the watch counts as using it, so the screen stays on while
                // you play, and sleeps normally once the watch is put down.
                if app_state == AppState::Fluid {
                    fluid_app.update(&input);
                    if fluid_app.is_moving() { last_interaction = now; }
                    fluid_app.render(&mut fb);
                    fb.flush(&mut display);
                } else {
                    let t_frame = Instant::now();
                    // gyro_data is stored as deg/s x10
                    waves_app.set_gyro(gyro_data.0 as f32 / 10.0, gyro_data.1 as f32 / 10.0, gyro_data.2 as f32 / 10.0);
                    waves_app.update(&input);
                    if waves_app.is_moving() { last_interaction = now; }
                    waves_app.render(&mut fb);
                    fb.flush(&mut display);
                    // Let Waves trim its particle count if the watch can't keep up.
                    waves_app.report_frame_ms((Instant::now() - t_frame).as_millis() as u32);
                }
                if boot_button.is_low() {
                    crate::peripherals::cpu_clock::set_cpu_mhz(watchface.cpu_mhz);
                    app_state = AppState::Launcher;
                    Timer::after(Duration::from_millis(200)).await;
                }
            }

            AppState::Settings => {
                use crate::apps::settings::WifiStatus;
                let mut changed = tap_event && settings_app.handle_tap(last_touch_x, last_touch_y);
                if settings_app.take_toggle() {
                    if wifi_on_request || wifi_connected {
                        wifi_on_request = false;
                    } else {
                        wifi_on_request = true;
                        wifi_failed = false;
                    }
                    last_interaction = now;
                    changed = true;
                }
                let status = if wifi_connected {
                    let ip = stack.config_v4().map(|c| c.address.address().octets());
                    WifiStatus::Connected { net: wifi_net, ip }
                } else if wifi_on_request {
                    WifiStatus::Connecting
                } else if wifi_failed {
                    WifiStatus::Failed
                } else {
                    WifiStatus::Off
                };
                settings_app.set_status(status);
                if changed || now >= next_watchface_flush {
                    settings_app.render(&mut fb);
                    fb.flush_vsync(&mut display, &te_pin);
                    next_watchface_flush = now + Duration::from_secs(1);
                }
                if boot_button.is_low() {
                    app_state = AppState::Launcher;
                    Timer::after(Duration::from_millis(200)).await;
                }
            }

            _ => {
                app_state = AppState::Watchface;
            }
        }
    }
}
