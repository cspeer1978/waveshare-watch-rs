// Battery: history from the flash battery log (peripherals/battlog.rs).
//   * Graph of battery % for the current charge (or the last 24 h when on USB),
//     with screen-on time drawn as small bars along the bottom.
//   * Summary: time on battery, % used, average drain, estimated full-charge
//     life, screen and Wi-Fi time.
//   * Past charges: how long each lasted and how it ended (died / recharged).
//   * Export the whole log as CSV:
//       WI-FI - the watch joins Wi-Fi and shows an address; open it in any
//               browser on the same network to get the CSV.
//       USB   - (only while plugged in) prints the CSV on the USB serial console.
//     A status line on screen says what happened.

use alloc::vec::Vec;
use core::fmt::Write;

use embedded_graphics::mono_font::ascii::FONT_8X13;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Alignment, Text};

use crate::apps::widgets::{self, Btn, SBuf};
use crate::apps::{App, AppInput, AppResult};
use crate::peripherals::battlog::{volt_pct, Entry, CAPACITY_MAH, F_BOOT, F_CHARGING, F_USB};

const GX0: i32 = 46;
const GX1: i32 = 368;
const GY0: i32 = 60;
const GY1: i32 = 192;
// Bottom buttons: Wi-Fi export always; USB export too while plugged in.
const WIFI_BTN_SOLO: Btn = Btn::new(115, 434, 180, 44);
const WIFI_BTN_PAIR: Btn = Btn::new(215, 434, 150, 44);
const USB_BTN: Btn = Btn::new(45, 434, 150, 44);

pub const CSV_HEADER: &str = "seq,date,time,pct,mv,volt_pct,screen_s,wifi_s,usb,charging,boot,sleep_s,awake_s,timer_wakes,touch_wakes,reset,pmu_on,pmu_off,boot_info,brownouts";

/// What the user asked for with the export buttons.
#[derive(Clone, Copy, PartialEq)]
pub enum Export { None, Usb, Wifi }

/// A stretch of time on battery.
#[derive(Clone, Copy)]
pub struct Session {
    pub start: usize, // index into entries
    pub end: usize,   // inclusive
    pub died: bool,   // ended by the battery running out (vs. plugged in)
    pub ongoing: bool,
}

pub struct BatteryApp {
    saved: Vec<Entry>,   // what's in the flash log
    entries: Vec<Entry>, // saved + one live "now" point at the end (what we draw)
    now_minute: u32,
    on_usb: bool,
    log_ok: bool,
    export: Export,
    status: SBuf<44>,
    status_color: Rgb565,
    crash: SBuf<36>,
}

/// On battery = no USB cable. (Older records also had a bogus "charging" flag
/// while on battery, so that flag is ignored here.)
fn on_battery(e: &Entry) -> bool { e.flags & F_USB == 0 }

/// The % we show: from voltage (calibrated on this watch's 400 mAh cell) while
/// on battery; the power chip's own % while plugged in (voltage reads high then).
fn shown_pct(e: &Entry) -> u8 { if on_battery(e) { volt_pct(e.mv) } else { e.pct } }

/// (year, month, day) from minutes since 2000-01-01 (Howard Hinnant's algorithm).
fn civil(minute: u32) -> (i64, u32, u32) {
    let z = (minute / 1440) as i64 + crate::apps::sunmoon::days_from_civil(2000, 1, 1) + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}
pub fn month_day(minute: u32) -> (u32, u32) { let (_, m, d) = civil(minute); (m, d) }
pub fn year_of(minute: u32) -> i64 { civil(minute).0 }

/// One CSV line (no newline) for a log record.
pub fn csv_line(e: &Entry) -> SBuf<160> {
    let (y, mo, d) = civil(e.minute);
    let mut b: SBuf<160> = SBuf::new();
    let _ = write!(b, "{},{}-{:02}-{:02},{:02}:{:02},{},{},{},{},{},{},{},{}",
        e.seq, y, mo, d, (e.minute % 1440) / 60, e.minute % 60, e.pct, e.mv, volt_pct(e.mv),
        if e.flags & F_BOOT != 0 { 0 } else { e.screen_s }, if e.flags & F_BOOT != 0 { 0 } else { e.radio_s },
        (e.flags & F_USB != 0) as u8,
        (e.flags & F_CHARGING != 0) as u8,
        (e.flags & F_BOOT != 0) as u8);
    if e.has_diag && e.flags & F_BOOT == 0 {
        let _ = write!(b, ",{},{},{},{}", e.sleep_s, e.awake_s, e.timer_wakes, e.gpio_wakes & 0x0F);
    } else {
        let _ = write!(b, ",,,,");
    }
    // Boot entries: why the watch started (ESP32-S3 reset code; 254 = after a crash).
    // ...and the power chip's power-on / power-off source registers (hex).
    if e.flags & F_BOOT != 0 {
        let _ = write!(b, ",{},{:02X},{:02X}", e.radio_s / 4, e.screen_s >> 8, e.screen_s & 0xFF);
        if e.has_diag {
            // swd = super-watchdog reset, bod = brown-out detected, mem = RTC memory kept
            // (a true power-on clears it), steps = last 4 loop steps before the restart
            let f = e.sleep_s;
            let _ = write!(b, ",swd{} bod{} mem{} steps {}-{}-{}-{}", f & 1, (f >> 1) & 1, (f >> 2) & 1,
                e.gpio_wakes, e.timer_wakes, e.awake_s >> 8, e.awake_s & 0xFF);
        }
    } else {
        let _ = write!(b, ",,,,");
    }
    if e.has_diag && e.flags & F_BOOT == 0 { let _ = write!(b, ",{}", e.gpio_wakes >> 4); } else { let _ = write!(b, ","); }
    b
}

fn fmt_dur(b: &mut SBuf<32>, minutes: u32) {
    if minutes >= 60 { let _ = write!(b, "{}h {:02}m", minutes / 60, minutes % 60); }
    else { let _ = write!(b, "{}m", minutes); }
}

impl BatteryApp {
    pub fn new() -> Self {
        Self {
            saved: Vec::new(), entries: Vec::new(), now_minute: 0, on_usb: false, log_ok: true,
            export: Export::None, status: SBuf::new(), status_color: Rgb565::CSS_DIM_GRAY,
            crash: SBuf::new(),
        }
    }

    /// The flash log contents (on open, and again whenever a new record is saved).
    pub fn load(&mut self, saved: Vec<Entry>, log_ok: bool) {
        self.saved = saved;
        self.log_ok = log_ok;
        self.rebuild(None);
    }

    /// Clears the export status line (when the app is opened).
    pub fn clear_status(&mut self) {
        self.status = SBuf::new();
        if !self.crash.as_str().is_empty() {
            // Until the next restart, opening Battery shows where the last crash was.
            let c = self.crash.as_str();
            let mut t: SBuf<44> = SBuf::new();
            let _ = write!(t, "Crashed: {}", c);
            self.status = t;
            self.status_color = widgets::CORAL_LIGHT;
        }
    }

    /// Message of a crash that restarted the watch (shown on the status line).
    pub fn set_crash(&mut self, msg: &str) {
        self.crash = SBuf::new();
        let _ = self.crash.write_str(msg);
    }

    /// Status line shown above the buttons (export progress / result).
    pub fn set_status(&mut self, text: &str, color: Rgb565) {
        self.status = SBuf::new();
        let _ = self.status.write_str(text);
        self.status_color = color;
    }

    /// Live reading from the power chip (every few seconds while open), so the
    /// screen shows the battery right now, not the last saved record.
    pub fn set_live(&mut self, now_minute: u32, pct: u8, mv: u16, charging: bool, on_usb: bool) {
        self.now_minute = now_minute;
        self.on_usb = on_usb;
        let mut flags = 0u8;
        if charging { flags |= F_CHARGING; }
        if on_usb { flags |= F_USB; }
        self.rebuild(Some(Entry { seq: u32::MAX, minute: now_minute, mv, pct, flags, ..Default::default() }));
    }

    fn rebuild(&mut self, live: Option<Entry>) {
        let live = live.or_else(|| self.entries.last().copied().filter(|e| e.seq == u32::MAX));
        self.entries.clear();
        self.entries.extend_from_slice(&self.saved);
        if let Some(mut l) = live {
            // Never let the live point go back in time (e.g. clock not set yet).
            if let Some(last) = self.saved.last() { l.minute = l.minute.max(last.minute); }
            self.entries.push(l);
        }
    }

    /// Saved records only (for export).
    pub fn entries(&self) -> &[Entry] { &self.saved }

    /// Which export was tapped (once; main does the work).
    pub fn take_export(&mut self) -> Export { core::mem::replace(&mut self.export, Export::None) }

    pub fn handle_tap(&mut self, x: u16, y: u16) -> bool {
        if self.on_usb {
            if USB_BTN.hit(x, y) { self.export = Export::Usb; return true; }
            if WIFI_BTN_PAIR.hit(x, y) { self.export = Export::Wifi; return true; }
        } else if WIFI_BTN_SOLO.hit(x, y) {
            self.export = Export::Wifi;
            return true;
        }
        false
    }

    /// Split the log into on-battery sessions.
    pub fn sessions(&self) -> Vec<Session> {
        let e = &self.entries;
        let mut out = Vec::new();
        let mut i = 0;
        while i < e.len() {
            if !on_battery(&e[i]) { i += 1; continue; }
            let start = i;
            let mut end = i;
            let mut died = false;
            while end + 1 < e.len() {
                let nx = &e[end + 1];
                if !on_battery(nx) { break; }
                // Clock jumped (backwards, or > 12 h ahead between two entries
                // that are ~10 min apart): the RTC was wrong, so start a new run.
                if nx.minute < e[end].minute || nx.minute > e[end].minute + 12 * 60 { break; }
                if nx.flags & F_BOOT != 0 && nx.minute > e[end].minute + 20 {
                    // Rebooted after a long silence while on battery: it ran out.
                    died = shown_pct(&e[end]) <= 10;
                    break;
                }
                end += 1;
            }
            let ongoing = end + 1 == e.len();
            if !ongoing && !died && end + 1 < e.len() {
                // Next record is USB/charging: it was plugged in (unless it was nearly empty
                // and there's a long gap before the next record — then it died first).
                let nx = &e[end + 1];
                died = shown_pct(&e[end]) <= 5 && nx.minute > e[end].minute + 30;
            }
            out.push(Session { start, end, died, ongoing });
            i = end + 1;
        }
        out
    }

    fn draw_graph<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D, from: usize, to: usize) {
        let small = MonoTextStyle::new(&FONT_8X13, Rgb565::CSS_GRAY);
        let grid = PrimitiveStyle::with_stroke(Rgb565::new(4, 8, 4), 1);
        for (pct, y) in [(100, GY0), (50, (GY0 + GY1) / 2), (0, GY1)] {
            let _ = Line::new(Point::new(GX0, y), Point::new(GX1, y)).into_styled(grid).draw(d);
            let mut s: SBuf<8> = SBuf::new();
            let _ = write!(s, "{}", pct);
            let _ = Text::with_alignment(s.as_str(), Point::new(GX0 - 4, y + 4), small, Alignment::Right).draw(d);
        }
        if to <= from { return; }
        let t0 = self.entries[from].minute;
        let t1 = self.entries[to].minute.max(t0 + 10);
        let xof = |m: u32| GX0 + ((m.max(t0) - t0) as i64 * (GX1 - GX0) as i64 / (t1 - t0) as i64) as i32;
        let yof = |p: u8| GY1 - (p.min(100) as i32 * (GY1 - GY0) / 100);
        // Screen-on bars along the bottom (height = share of the interval screen was on).
        for k in from + 1..=to {
            let e = &self.entries[k];
            if e.screen_s == 0 || e.flags & F_BOOT != 0 { continue; }
            let span = (e.minute.saturating_sub(self.entries[k - 1].minute)).max(1) * 60;
            let h = ((e.screen_s as u32 * 30) / span).clamp(1, 30) as i32;
            let (xa, xb) = (xof(self.entries[k - 1].minute), xof(e.minute));
            let _ = Rectangle::new(Point::new(xa, GY1 - h), Size::new((xb - xa).max(1) as u32, h as u32))
                .into_styled(PrimitiveStyle::with_fill(widgets::AMBER)).draw(d);
        }
        let line = PrimitiveStyle::with_stroke(widgets::GREEN_LIGHT, 3);
        for k in from + 1..=to {
            let (a, b) = (&self.entries[k - 1], &self.entries[k]);
            let col = if on_battery(b) { line } else { PrimitiveStyle::with_stroke(widgets::TEAL_LIGHT, 3) };
            let _ = Line::new(Point::new(xof(a.minute), yof(shown_pct(a))), Point::new(xof(b.minute), yof(shown_pct(b)))).into_styled(col).draw(d);
        }
    }

    fn draw_bottom<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        // Status line: export progress, or the log health line.
        let mut t: SBuf<44> = SBuf::new();
        let color = if !self.status.as_str().is_empty() {
            let _ = t.write_str(self.status.as_str());
            self.status_color
        } else if self.log_ok {
            let _ = write!(t, "{} saved - 1 every 10 min", self.saved.len());
            Rgb565::CSS_DIM_GRAY
        } else {
            let _ = t.write_str("LOG ERROR - not saving");
            widgets::CORAL_LIGHT
        };
        let st = MonoTextStyle::new(&FONT_8X13, color);
        let _ = Text::with_alignment(t.as_str(), Point::new(205, 420), st, Alignment::Center).draw(d);
        if self.on_usb {
            USB_BTN.draw(d, "USB", widgets::TEAL, Rgb565::WHITE);
            WIFI_BTN_PAIR.draw(d, "WI-FI", widgets::AMBER, Rgb565::WHITE);
        } else {
            WIFI_BTN_SOLO.draw(d, "EXPORT", widgets::AMBER, Rgb565::WHITE);
        }
    }
}

impl App for BatteryApp {
    fn name(&self) -> &str { "Battery" }
    fn setup(&mut self) {}
    fn update(&mut self, _input: &AppInput) -> AppResult { AppResult::Continue }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        widgets::clear(d, Rgb565::BLACK);
        widgets::title(d, "BATTERY", widgets::GREEN_LIGHT);
        if self.entries.is_empty() {
            widgets::text_center(d, "No history yet", 200, Rgb565::WHITE);
            widgets::text_center(d, "An entry is saved about", 240, Rgb565::CSS_GRAY);
            widgets::text_center(d, "every 10 minutes", 264, Rgb565::CSS_GRAY);
            return;
        }
        let sessions = self.sessions();
        let cur = sessions.last().copied().filter(|s| s.ongoing && !self.on_usb);
        let last = self.entries.len() - 1;

        // Graph: current charge, or the last 24 h when on USB / no session.
        let (gfrom, gto) = match cur {
            Some(s) if s.end > s.start => (s.start, s.end),
            _ => {
                let cutoff = self.entries[last].minute.saturating_sub(24 * 60);
                (self.entries.iter().position(|e| e.minute >= cutoff).unwrap_or(0), last)
            }
        };
        self.draw_graph(d, gfrom, gto);

        let mut y = 226;
        let line = |d: &mut D, s: &str, y: i32, c: Rgb565| widgets::text_center(d, s, y, c);
        match cur {
            Some(s) => {
                let (a, b) = (&self.entries[s.start], &self.entries[s.end]);
                let mins = self.now_minute.max(b.minute).saturating_sub(a.minute).min(14 * 24 * 60);
                let mut t: SBuf<32> = SBuf::new();
                let _ = write!(t, "On battery ");
                fmt_dur(&mut t, mins);
                line(d, t.as_str(), y, Rgb565::WHITE); y += 26;
                let (pa, pb) = (shown_pct(a), shown_pct(b));
                let used = pa.saturating_sub(pb) as u32;
                let mut t: SBuf<32> = SBuf::new();
                if mins >= 20 && used > 0 {
                    let rate10 = (used * 600 / mins).max(1); // %/h x10
                    let ma = rate10 * CAPACITY_MAH / 1000;   // average mA on the 400 mAh cell
                    let _ = write!(t, "{}% -> {}%  ({}.{}%/h {}mA)", pa, pb, rate10 / 10, rate10 % 10, ma);
                    line(d, t.as_str(), y, Rgb565::CSS_LIGHT_GRAY); y += 26;
                    let mut t: SBuf<32> = SBuf::new();
                    let _ = write!(t, "~");
                    fmt_dur(&mut t, pb as u32 * 600 / rate10);
                    let _ = write!(t, " left, full ~");
                    let full = 100 * 600 / rate10;
                    let _ = write!(t, "{}h", (full + 30) / 60);
                    line(d, t.as_str(), y, widgets::GREEN_LIGHT); y += 26;
                } else {
                    let _ = write!(t, "{}% -> {}%  (measuring...)", pa, pb);
                    line(d, t.as_str(), y, Rgb565::CSS_LIGHT_GRAY); y += 26;
                }
                let seg = &self.entries[s.start..=s.end];
                let scr: u32 = seg.iter().filter(|e| e.flags & F_BOOT == 0).map(|e| e.screen_s as u32).sum();
                let rad: u32 = seg.iter().map(|e| e.radio_s as u32).sum();
                let mut t: SBuf<32> = SBuf::new();
                let _ = write!(t, "Screen ");
                fmt_dur(&mut t, scr / 60);
                let _ = write!(t, "   Wi-Fi ");
                fmt_dur(&mut t, rad / 60);
                line(d, t.as_str(), y, widgets::AMBER_LIGHT); y += 26;
                // Sleep diagnostics: share of time in light sleep, wakes per hour.
                let (mut sl, mut aw, mut wk, mut sc2) = (0u32, 0u32, 0u32, 0u32);
                for e in seg.iter().filter(|e| e.has_diag && e.flags & F_BOOT == 0) {
                    sl += e.sleep_s as u32; aw += e.awake_s as u32; if e.flags & F_BOOT == 0 { sc2 += e.screen_s as u32; }
                    wk += e.timer_wakes as u32 + (e.gpio_wakes & 0x0F) as u32;
                }
                let tot = sl + aw + sc2;
                if tot >= 600 {
                    let mut t: SBuf<32> = SBuf::new();
                    let _ = write!(t, "Asleep {}%  {} wakes/h", sl * 100 / tot, wk * 3600 / tot);
                    line(d, t.as_str(), y, widgets::TEAL_LIGHT); y += 26;
                }
                y += 4;
            }
            None => {
                let e = &self.entries[last];
                let mut t: SBuf<32> = SBuf::new();
                let what = if !self.on_usb { "On battery" } else if e.flags & F_CHARGING != 0 { "Charging" } else { "Plugged in" };
                let _ = write!(t, "{} {}%  {}mV", what, shown_pct(e), e.mv);
                line(d, t.as_str(), y, Rgb565::WHITE); y += 32;
            }
        }

        // Past charges (most recent first; as many as fit above the status line)
        let past: Vec<&Session> = sessions.iter().rev()
            .filter(|s| !s.ongoing || self.on_usb)
            // skip blips (e.g. a minute unplugged while flashing)
            .filter(|s| self.entries[s.end].minute >= self.entries[s.start].minute + 10)
            .take(4).collect();
        if !past.is_empty() && y < 370 {
            line(d, "Past charges", y, Rgb565::CSS_GRAY); y += 22;
            for s in past {
                if y > 396 { break; }
                let (a, b) = (&self.entries[s.start], &self.entries[s.end]);
                let (m, dd) = month_day(a.minute);
                let mut t: SBuf<32> = SBuf::new();
                let _ = write!(t, "{}/{} ", m, dd);
                fmt_dur(&mut t, b.minute.saturating_sub(a.minute));
                let _ = write!(t, " {}-{}% {}", shown_pct(a), shown_pct(b), if s.died { "died" } else { "chg" });
                let st = MonoTextStyle::new(&FONT_8X13, if s.died { widgets::CORAL_LIGHT } else { Rgb565::CSS_LIGHT_GRAY });
                let _ = Text::with_alignment(t.as_str(), Point::new(205, y), st, Alignment::Center).draw(d);
                y += 19;
            }
        }

        self.draw_bottom(d);
    }
}
