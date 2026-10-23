// Settings: Wi-Fi network choice.
//
//   Auto        - try the home network first, fall back to the phone hotspot
//   <home>      - only the home network   (WIFI_SSID / WIFI_PASS at build time)
//   <hotspot>   - only the phone hotspot  (WIFI2_SSID / WIFI2_PASS at build time)
//
// Below the choices: what the radio is doing right now (off / connecting /
// connected to which network, with the watch's IP address) and a
// CONNECT / DISCONNECT button. Main does the actual radio work; this screen
// only records what the user asked for.
// (The choice is kept until the watch restarts; it then goes back to Auto.)

use core::fmt::Write;

use embedded_graphics::mono_font::ascii::{FONT_10X20, FONT_8X13};
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle, RoundedRectangle};
use embedded_graphics::text::{Alignment, Text};

use crate::apps::widgets::{self, Btn, SBuf};

pub const MODE_AUTO: u8 = 0;
pub const MODE_HOME: u8 = 1;
pub const MODE_HOTSPOT: u8 = 2;

/// Network names compiled into the image (empty if not set at build time).
pub const HOME_SSID: &str = match option_env!("WIFI_SSID") { Some(s) => s, None => "" };
pub const HOTSPOT_SSID: &str = match option_env!("WIFI2_SSID") { Some(s) => s, None => "" };

const ROW_X: i32 = 30;
const ROW_W: i32 = 350;
const ROW_H: i32 = 50;
const ROW_Y: [i32; 3] = [84, 144, 204];
const CONNECT_BTN: Btn = Btn::new(105, 370, 200, 50);

/// What the Wi-Fi radio is doing (set by main every frame).
#[derive(Clone, Copy, PartialEq)]
pub enum WifiStatus {
    Off,
    Connecting,
    Connected { net: u8, ip: Option<[u8; 4]> }, // net: MODE_HOME / MODE_HOTSPOT
    Failed,
}

pub struct SettingsApp {
    pub mode: u8,
    status: WifiStatus,
    toggle: bool,    // CONNECT / DISCONNECT tapped
    reconnect: bool, // network choice changed while connected
}

fn net_name(net: u8) -> &'static str {
    match net {
        MODE_HOME => if HOME_SSID.is_empty() { "(home not set)" } else { HOME_SSID },
        MODE_HOTSPOT => if HOTSPOT_SSID.is_empty() { "(hotspot not set)" } else { HOTSPOT_SSID },
        _ => "Auto",
    }
}

impl SettingsApp {
    pub fn new() -> Self {
        Self { mode: MODE_AUTO, status: WifiStatus::Off, toggle: false, reconnect: false }
    }

    pub fn set_status(&mut self, s: WifiStatus) { self.status = s; }

    /// CONNECT / DISCONNECT was tapped (once).
    pub fn take_toggle(&mut self) -> bool { core::mem::take(&mut self.toggle) }

    /// The network choice changed while connected: reconnect with the new choice (once).
    pub fn take_reconnect(&mut self) -> bool { core::mem::take(&mut self.reconnect) }

    /// Order to try networks in for the current choice.
    pub fn candidates(&self) -> &'static [u8] {
        match self.mode {
            MODE_HOME => &[MODE_HOME],
            MODE_HOTSPOT => &[MODE_HOTSPOT],
            _ => &[MODE_HOME, MODE_HOTSPOT],
        }
    }

    /// Handle a tap. Returns true if something changed (redraw).
    pub fn handle_tap(&mut self, x: u16, y: u16) -> bool {
        let (xi, yi) = (x as i32, y as i32);
        for (i, &ry) in ROW_Y.iter().enumerate() {
            if xi >= ROW_X - 6 && xi < ROW_X + ROW_W + 6 && yi >= ry - 4 && yi < ry + ROW_H + 4 {
                let m = i as u8;
                if m != self.mode {
                    self.mode = m;
                    if matches!(self.status, WifiStatus::Connected { .. }) {
                        // Reconnect only if the current network isn't allowed any more.
                        if let WifiStatus::Connected { net, .. } = self.status {
                            if !self.candidates().contains(&net) || self.candidates()[0] != net {
                                self.reconnect = true;
                            }
                        }
                    }
                }
                return true;
            }
        }
        if CONNECT_BTN.hit(x, y) {
            self.toggle = true;
            return true;
        }
        false
    }

    pub fn update(&mut self, _dt_ms: u32) {}

    pub fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        widgets::clear(d, Rgb565::BLACK);
        widgets::title(d, "SETTINGS", widgets::TEAL_LIGHT);
        let small = MonoTextStyle::new(&FONT_8X13, Rgb565::CSS_GRAY);
        let _ = Text::with_alignment("Wi-Fi network", Point::new(205, 70), small, Alignment::Center).draw(d);

        let labels: [SBuf<32>; 3] = {
            let mut a: SBuf<32> = SBuf::new();
            let _ = write!(a, "Auto ({} first)", net_name(MODE_HOME));
            let mut b: SBuf<32> = SBuf::new();
            let _ = b.write_str(net_name(MODE_HOME));
            let mut c: SBuf<32> = SBuf::new();
            let _ = c.write_str(net_name(MODE_HOTSPOT));
            [a, b, c]
        };
        for (i, &ry) in ROW_Y.iter().enumerate() {
            let sel = self.mode == i as u8;
            let bg = if sel { widgets::TEAL } else { Rgb565::new(3, 6, 4) };
            let _ = RoundedRectangle::with_equal_corners(
                Rectangle::new(Point::new(ROW_X, ry), Size::new(ROW_W as u32, ROW_H as u32)),
                Size::new(10, 10),
            ).into_styled(PrimitiveStyle::with_fill(bg)).draw(d);
            // radio dot
            let dot = embedded_graphics::primitives::Circle::with_center(Point::new(ROW_X + 26, ry + ROW_H / 2), 16);
            let _ = dot.into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 2)).draw(d);
            if sel {
                let _ = embedded_graphics::primitives::Circle::with_center(Point::new(ROW_X + 26, ry + ROW_H / 2), 8)
                    .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE)).draw(d);
            }
            let st = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
            let _ = Text::new(labels[i].as_str(), Point::new(ROW_X + 48, ry + ROW_H / 2 + 6), st).draw(d);
        }

        // Status
        let mut s1: SBuf<40> = SBuf::new();
        let mut s2: SBuf<40> = SBuf::new();
        let color = match self.status {
            WifiStatus::Off => { let _ = s1.write_str("Wi-Fi is off"); Rgb565::CSS_GRAY }
            WifiStatus::Connecting => { let _ = s1.write_str("Connecting..."); widgets::AMBER_LIGHT }
            WifiStatus::Failed => {
                let _ = s1.write_str("Couldn't connect");
                let _ = s2.write_str("Out of range or wrong password?");
                widgets::CORAL_LIGHT
            }
            WifiStatus::Connected { net, ip } => {
                let _ = write!(s1, "On {}", net_name(net));
                match ip {
                    Some(a) => { let _ = write!(s2, "IP {}.{}.{}.{}", a[0], a[1], a[2], a[3]); }
                    None => { let _ = s2.write_str("getting address..."); }
                }
                widgets::GREEN_LIGHT
            }
        };
        widgets::text_center(d, s1.as_str(), 302, color);
        if !s2.as_str().is_empty() {
            let _ = Text::with_alignment(s2.as_str(), Point::new(205, 328), MonoTextStyle::new(&FONT_8X13, Rgb565::CSS_LIGHT_GRAY), Alignment::Center).draw(d);
        }

        let (label, bg) = match self.status {
            WifiStatus::Connected { .. } | WifiStatus::Connecting => ("DISCONNECT", widgets::CORAL),
            _ => ("CONNECT", widgets::GREEN),
        };
        CONNECT_BTN.draw(d, label, bg, Rgb565::WHITE);
        let _ = Text::with_alignment("Wi-Fi turns off after 5 min idle", Point::new(205, 446), small, Alignment::Center).draw(d);
        widgets::text_center(d, "BOOT = back", 480, Rgb565::CSS_DIM_GRAY);
    }
}
