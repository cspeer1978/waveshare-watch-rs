// Alarm clock: one daily alarm, set with H-/H+/M-/M+, ON/OFF toggle.
// The main loop does the hardware side:
//   * checks the RTC once a second (and once per AOD/sleep wake),
//   * tells light sleep to wake up exactly at the alarm time (secs_until),
//   * when check() fires: wakes the screen, opens this app, and beeps every
//     ~1.5 s until STOP / SNOOZE (or 2 minutes pass).

use core::fmt::Write;

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;

use crate::apps::widgets::{self, Btn, SBuf, CX};
use crate::apps::{App, AppInput, AppResult};
use crate::ui::segments;

const H_MINUS: Btn = Btn::new(30, 230, 80, 60);
const H_PLUS: Btn = Btn::new(118, 230, 80, 60);
const M_MINUS: Btn = Btn::new(212, 230, 80, 60);
const M_PLUS: Btn = Btn::new(300, 230, 80, 60);
const TOGGLE: Btn = Btn::new(80, 320, 250, 70);
// While ringing
const STOP_BTN: Btn = Btn::new(30, 300, 165, 90);
const SNOOZE_BTN: Btn = Btn::new(215, 300, 165, 90);

pub const SNOOZE_MIN: u16 = 9;
/// Stop ringing on its own after this long.
pub const RING_SECS: u64 = 120;

pub struct AlarmApp {
    hour: u8,   // 0..23
    minute: u8, // 0..59
    enabled: bool,
    ringing: bool,
    snooze_at: Option<u16>,     // minutes of day
    last_fired: Option<u16>,    // minute of day we last fired at (avoid re-firing)
}

impl AlarmApp {
    pub fn new() -> Self {
        Self { hour: 7, minute: 0, enabled: false, ringing: false, snooze_at: None, last_fired: None }
    }

    fn target(&self) -> Option<u16> {
        if !self.enabled { return None; }
        Some(self.snooze_at.unwrap_or(self.hour as u16 * 60 + self.minute as u16))
    }

    /// True if an alarm (or snooze) is set, i.e. the main loop must keep checking.
    pub fn is_armed(&self) -> bool { self.target().is_some() }
    pub fn is_ringing(&self) -> bool { self.ringing }

    /// Seconds from now (RTC h:m:s) until the alarm should go off.
    pub fn secs_until(&self, h: u8, m: u8, s: u8) -> Option<u32> {
        let t = self.target()? as i32 * 60;
        let now = h as i32 * 3600 + m as i32 * 60 + s as i32;
        let mut d = t - now;
        if d <= 0 { d += 86_400; }
        Some(d as u32)
    }

    /// Call with the current RTC time. Returns true once when the alarm should start ringing.
    pub fn check(&mut self, h: u8, m: u8) -> bool {
        let now = h as u16 * 60 + m as u16;
        match self.target() {
            Some(t) if t == now && !self.ringing && self.last_fired != Some(now) => {
                self.ringing = true;
                self.last_fired = Some(now);
                self.snooze_at = None;
                true
            }
            _ => {
                // Allow the same time to fire again tomorrow.
                if self.last_fired.map_or(false, |f| f != now) { self.last_fired = None; }
                false
            }
        }
    }

    pub fn stop(&mut self) {
        self.ringing = false;
        self.snooze_at = None;
    }

    fn snooze(&mut self, h: u8, m: u8) {
        self.ringing = false;
        self.snooze_at = Some((h as u16 * 60 + m as u16 + SNOOZE_MIN) % 1440);
    }

    /// Returns true if the tap changed something. Needs the current time for snooze.
    pub fn handle_tap(&mut self, x: u16, y: u16, now_h: u8, now_m: u8) -> bool {
        if self.ringing {
            if STOP_BTN.hit(x, y) { self.stop(); return true; }
            if SNOOZE_BTN.hit(x, y) { self.snooze(now_h, now_m); return true; }
            return false;
        }
        let mut changed = true;
        if H_MINUS.hit(x, y) { self.hour = (self.hour + 23) % 24; }
        else if H_PLUS.hit(x, y) { self.hour = (self.hour + 1) % 24; }
        else if M_MINUS.hit(x, y) { self.minute = (self.minute + 55) % 60; }
        else if M_PLUS.hit(x, y) { self.minute = (self.minute + 5) % 60; }
        else if TOGGLE.hit(x, y) { self.enabled = !self.enabled; self.snooze_at = None; }
        else { changed = false; }
        if changed && !TOGGLE.hit(x, y) {
            // Editing the time clears any pending snooze and re-arms for today.
            self.snooze_at = None;
            self.last_fired = None;
        }
        changed
    }
}

impl App for AlarmApp {
    fn name(&self) -> &str { "Alarm" }
    fn setup(&mut self) {}
    fn update(&mut self, _input: &AppInput) -> AppResult { AppResult::Continue }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        widgets::clear(d, Rgb565::BLACK);
        let shown = self.snooze_at.map(|t| ((t / 60) as u8, (t % 60) as u8)).unwrap_or((self.hour, self.minute));
        let pm = shown.0 >= 12;

        if self.ringing {
            widgets::title(d, "ALARM!", widgets::CORAL_LIGHT);
            let _ = segments::draw_hhmm_sized(d, CX, 80, segments::to_12h(shown.0), shown.1,
                56, 100, 9, 8, 18, Rgb565::WHITE);
            widgets::text_center(d, if pm { "PM" } else { "AM" }, 215, widgets::AMBER_LIGHT);
            STOP_BTN.draw(d, "STOP", widgets::CORAL, Rgb565::WHITE);
            let mut s: SBuf<16> = SBuf::new();
            let _ = write!(s, "SNOOZE {}m", SNOOZE_MIN);
            SNOOZE_BTN.draw(d, s.as_str(), widgets::AMBER, Rgb565::WHITE);
            return;
        }

        widgets::title(d, "ALARM", widgets::AMBER_LIGHT);
        let col = if self.enabled { Rgb565::WHITE } else { Rgb565::CSS_GRAY };
        let _ = segments::draw_hhmm_sized(d, CX, 72, segments::to_12h(shown.0), shown.1,
            50, 90, 8, 8, 16, col);
        widgets::text_center(d, if pm { "PM" } else { "AM" }, 198, widgets::AMBER_LIGHT);

        H_MINUS.draw(d, "H-", widgets::TEAL, Rgb565::WHITE);
        H_PLUS.draw(d, "H+", widgets::TEAL, Rgb565::WHITE);
        M_MINUS.draw(d, "M-", widgets::GREEN, Rgb565::WHITE);
        M_PLUS.draw(d, "M+", widgets::GREEN, Rgb565::WHITE);
        let (lbl, bg) = if self.enabled { ("ALARM ON", widgets::AMBER) } else { ("ALARM OFF", widgets::SLATE) };
        TOGGLE.draw(d, lbl, bg, Rgb565::WHITE);

        let hint = if self.snooze_at.is_some() { "Snoozed" } else if self.enabled { "Rings daily at this time" } else { "Tap to turn on" };
        widgets::text_center(d, hint, 425, Rgb565::CSS_GRAY);
        widgets::text_center(d, "BOOT = back", 480, Rgb565::CSS_DIM_GRAY);
    }
}
