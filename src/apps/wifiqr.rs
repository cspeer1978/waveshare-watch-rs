// Wi-Fi QR: a scannable code so guests can join the home Wi-Fi.
// Built from the same WIFI_SSID / WIFI_PASS the firmware is compiled with,
// so it updates automatically if the Wi-Fi details change.
// Standard format understood by iPhone and Android cameras:
//   WIFI:T:WPA;S:<ssid>;P:<password>;;

use alloc::string::String;
use alloc::vec::Vec;

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use qrcodegen_no_heap::{QrCode, QrCodeEcc, Version};

use crate::apps::widgets::{self, CX};
use crate::apps::{App, AppInput, AppResult};

pub const SSID: &str = match option_env!("WIFI_SSID") { Some(s) => s, None => "" };
pub const PASS: &str = match option_env!("WIFI_PASS") { Some(s) => s, None => "" };
const BOX: i32 = 300;   // QR area incl. white border, px
const TOP: i32 = 58;

pub struct WifiQrApp {
    size: i32,
    modules: Vec<bool>, // size*size, row-major; true = dark
}

/// Escape \ ; , : " as the Wi-Fi QR format requires.
fn esc(out: &mut String, s: &str) {
    for c in s.chars() {
        if matches!(c, '\\' | ';' | ',' | ':' | '"') { out.push('\\'); }
        out.push(c);
    }
}

impl WifiQrApp {
    pub fn new() -> Self { Self { size: 0, modules: Vec::new() } }

    pub fn payload() -> String {
        let mut s = String::from("WIFI:T:");
        s.push_str(if PASS.is_empty() { "nopass" } else { "WPA" });
        s.push_str(";S:");
        esc(&mut s, SSID);
        s.push_str(";P:");
        esc(&mut s, PASS);
        s.push_str(";;");
        s
    }

    fn build(&mut self) {
        if !self.modules.is_empty() || SSID.is_empty() { return; }
        let text = Self::payload();
        let ver = Version::new(10);
        let mut tmp = alloc::vec![0u8; ver.buffer_len()];
        let mut out = alloc::vec![0u8; ver.buffer_len()];
        if let Ok(qr) = QrCode::encode_text(&text, &mut tmp, &mut out, QrCodeEcc::Medium, Version::MIN, ver, None, true) {
            self.size = qr.size();
            self.modules = (0..self.size * self.size).map(|i| qr.get_module(i % self.size, i / self.size)).collect();
        }
    }
}

impl App for WifiQrApp {
    fn name(&self) -> &str { "Wi-Fi QR" }
    fn setup(&mut self) { self.build(); }
    fn update(&mut self, _input: &AppInput) -> AppResult { AppResult::Continue }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        widgets::clear(d, Rgb565::BLACK);
        widgets::title(d, "JOIN WI-FI", widgets::GREEN_LIGHT);
        if self.size == 0 {
            widgets::text_center(d, "No Wi-Fi details in build", 240, Rgb565::YELLOW);
            return;
        }
        // White quiet zone of 4 modules around the code, scaled to fit BOX.
        let total = self.size + 8;
        let px = BOX / total;
        let side = px * total;
        let x0 = CX - side / 2;
        let _ = Rectangle::new(Point::new(x0, TOP), Size::new(side as u32, side as u32))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE)).draw(d);
        let dark = PrimitiveStyle::with_fill(Rgb565::BLACK);
        for y in 0..self.size {
            for x in 0..self.size {
                if self.modules[(y * self.size + x) as usize] {
                    let _ = Rectangle::new(Point::new(x0 + (x + 4) * px, TOP + (y + 4) * px), Size::new(px as u32, px as u32))
                        .into_styled(dark).draw(d);
                }
            }
        }
        let ty = TOP + side + 30;
        let mut s = String::from("Network: ");
        s.push_str(SSID);
        widgets::text_center(d, &s, ty, Rgb565::WHITE);
        let mut s = String::from("Password: ");
        s.push_str(PASS);
        widgets::text_center(d, &s, ty + 28, Rgb565::CSS_LIGHT_GRAY);
        widgets::text_center(d, "Point a phone camera here", ty + 60, Rgb565::CSS_GRAY);
    }
}
