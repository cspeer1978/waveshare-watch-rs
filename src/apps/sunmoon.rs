// Sun & Moon: today's sunrise / sunset / daylight for a fixed location, and
// the moon phase (drawn) with % illuminated and days to the next full moon.
// Pure math from the RTC date — no network needed.

use core::fmt::Write;

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, Line, PrimitiveStyle};

use crate::apps::widgets::{self, SBuf, CX};
use crate::apps::{App, AppInput, AppResult};

// ===== CONFIG =====
/// Location for sunrise/sunset (Bayonne, NJ).
pub const LAT: f64 = 40.669;
pub const LON: f64 = -74.114; // east positive
// ===================

const SYNODIC: f64 = 29.530588853;
const NEW_MOON_JD: f64 = 2451550.26; // 2000-01-06 18:14 UTC

const MOON_CY: i32 = 268;
const MOON_R: i32 = 72;

pub struct SunMoonApp {
    year: i32,
    month: u32,
    day: u32,
    rise_min: Option<i32>, // local minutes of day
    set_min: Option<i32>,
    moon_age: f64,         // days since new moon
}

/// Days from 1970-01-01 for a civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// 0 = Sunday.
pub fn weekday(y: i32, m: u32, d: u32) -> u32 {
    ((days_from_civil(y, m, d) + 4).rem_euclid(7)) as u32
}

/// US daylight saving (2nd Sunday of March .. 1st Sunday of November), by date.
pub fn us_dst(y: i32, m: u32, d: u32) -> bool {
    match m {
        4..=10 => true,
        3 => { let first_sun = 1 + (7 - weekday(y, 3, 1)) % 7; d >= first_sun + 7 }
        11 => { let first_sun = 1 + (7 - weekday(y, 11, 1)) % 7; d < first_sun }
        _ => false,
    }
}

/// Positive modulo for floats (f64::rem_euclid needs std).
fn fmodp(a: f64, b: f64) -> f64 { a - b * libm::floor(a / b) }

fn rad(x: f64) -> f64 { x * core::f64::consts::PI / 180.0 }

/// Sunrise/sunset in UTC minutes of day (Wikipedia "sunrise equation").
fn sun_times_utc(jd_noon: f64) -> Option<(f64, f64)> {
    let n = libm::round(jd_noon - 2451545.0 + 0.0008);
    let j_star = n - LON / 360.0;
    let m = fmodp(357.5291 + 0.98560028 * j_star, 360.0);
    let c = 1.9148 * libm::sin(rad(m)) + 0.02 * libm::sin(rad(2.0 * m)) + 0.0003 * libm::sin(rad(3.0 * m));
    let lambda = fmodp(m + c + 180.0 + 102.9372, 360.0);
    let j_transit = 2451545.0 + j_star + 0.0053 * libm::sin(rad(m)) - 0.0069 * libm::sin(rad(2.0 * lambda));
    let sin_d = libm::sin(rad(lambda)) * libm::sin(rad(23.4397));
    let cos_d = libm::cos(libm::asin(sin_d));
    let cos_w = (libm::sin(rad(-0.833)) - libm::sin(rad(LAT)) * sin_d) / (libm::cos(rad(LAT)) * cos_d);
    if !(-1.0..=1.0).contains(&cos_w) { return None; }
    let w = libm::acos(cos_w) * 180.0 / core::f64::consts::PI;
    let to_min = |j: f64| fmodp(j + 0.5, 1.0) * 1440.0;
    Some((to_min(j_transit - w / 360.0), to_min(j_transit + w / 360.0)))
}

impl SunMoonApp {
    pub fn new() -> Self {
        Self { year: 2026, month: 1, day: 1, rise_min: None, set_min: None, moon_age: 0.0 }
    }

    /// Recompute for a local date (from the RTC). `hour` refines the moon age.
    pub fn set_date(&mut self, year: i32, month: u32, day: u32, hour: u32) {
        self.year = year; self.month = month; self.day = day;
        let jd0 = days_from_civil(year, month, day) as f64 + 2440587.5; // 00:00 UTC
        let offset_h = if us_dst(year, month, day) { -4.0 } else { -5.0 };
        self.rise_min = None; self.set_min = None;
        if let Some((r, s)) = sun_times_utc(jd0 + 0.5) {
            let loc = |u: f64| fmodp(u + offset_h * 60.0, 1440.0) as i32;
            self.rise_min = Some(loc(r));
            self.set_min = Some(loc(s));
        }
        let jd_now = jd0 + (hour as f64 - offset_h) / 24.0;
        self.moon_age = fmodp(jd_now - NEW_MOON_JD, SYNODIC);
    }

    pub fn rise_set(&self) -> (Option<i32>, Option<i32>) { (self.rise_min, self.set_min) }

    pub fn phase_name(&self) -> &'static str {
        let a = self.moon_age;
        if a < 1.84 { "New Moon" } else if a < 5.53 { "Waxing Crescent" } else if a < 9.22 { "First Quarter" }
        else if a < 12.91 { "Waxing Gibbous" } else if a < 16.61 { "Full Moon" } else if a < 20.30 { "Waning Gibbous" }
        else if a < 23.99 { "Last Quarter" } else if a < 27.68 { "Waning Crescent" } else { "New Moon" }
    }

    pub fn illumination(&self) -> f64 {
        (1.0 - libm::cos(2.0 * core::f64::consts::PI * self.moon_age / SYNODIC)) / 2.0
    }

    fn fmt_time(b: &mut SBuf<24>, label: &str, min: Option<i32>) {
        match min {
            Some(t) => {
                let h = (t / 60) as u8;
                let _ = write!(b, "{} {}:{:02} {}", label, crate::ui::segments::to_12h(h), t % 60, if h >= 12 { "PM" } else { "AM" });
            }
            None => { let _ = write!(b, "{} --", label); }
        }
    }

    fn draw_moon<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        let dark = Rgb565::new(4, 8, 5);
        let lit = Rgb565::new(30, 58, 22);
        let ang = 2.0 * core::f64::consts::PI * self.moon_age / SYNODIC;
        let k = libm::cos(ang);
        let waxing = self.moon_age < SYNODIC / 2.0;
        for dy in -MOON_R..=MOON_R {
            let w = libm::sqrt((MOON_R * MOON_R - dy * dy) as f64);
            let y = MOON_CY + dy;
            let l = CX - w as i32;
            let r = CX + w as i32;
            let _ = Line::new(Point::new(l, y), Point::new(r, y)).into_styled(PrimitiveStyle::with_stroke(dark, 1)).draw(d);
            // Lit part: waxing lights the right side, waning the left (northern hemisphere).
            let (a, b) = if waxing { ((w * k) as i32, w as i32) } else { (-(w as i32), (-w * k) as i32) };
            if b > a {
                let _ = Line::new(Point::new(CX + a, y), Point::new(CX + b, y)).into_styled(PrimitiveStyle::with_stroke(lit, 1)).draw(d);
            }
        }
        let _ = Circle::with_center(Point::new(CX, MOON_CY), (MOON_R * 2 + 2) as u32)
            .into_styled(PrimitiveStyle::with_stroke(Rgb565::new(10, 20, 10), 1)).draw(d);
    }
}

impl App for SunMoonApp {
    fn name(&self) -> &str { "Sun & Moon" }
    fn setup(&mut self) {}
    fn update(&mut self, _input: &AppInput) -> AppResult { AppResult::Continue }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        widgets::clear(d, Rgb565::BLACK);
        widgets::title(d, "SUN & MOON", widgets::AMBER_LIGHT);
        let mut b: SBuf<24> = SBuf::new();
        Self::fmt_time(&mut b, "Sunrise", self.rise_min);
        widgets::text_center(d, b.as_str(), 82, widgets::AMBER_LIGHT);
        let mut b: SBuf<24> = SBuf::new();
        Self::fmt_time(&mut b, "Sunset ", self.set_min);
        widgets::text_center(d, b.as_str(), 112, widgets::CORAL_LIGHT);
        if let (Some(r), Some(s)) = (self.rise_min, self.set_min) {
            let len = s - r;
            let mut b: SBuf<24> = SBuf::new();
            let _ = write!(b, "Daylight {}h {:02}m", len / 60, len % 60);
            widgets::text_center(d, b.as_str(), 142, Rgb565::CSS_GRAY);
        }

        self.draw_moon(d);
        widgets::text_center(d, self.phase_name(), 380, Rgb565::WHITE);
        let mut b: SBuf<24> = SBuf::new();
        let _ = write!(b, "{}% lit", libm::round(self.illumination() * 100.0) as i32);
        widgets::text_center(d, b.as_str(), 408, Rgb565::CSS_LIGHT_GRAY);
        let to_full = fmodp(SYNODIC / 2.0 - self.moon_age, SYNODIC);
        let mut b: SBuf<32> = SBuf::new();
        let days = libm::round(to_full) as i32;
        if days == 0 { let _ = write!(b, "Full moon today"); }
        else { let _ = write!(b, "Full moon in {} day{}", days, if days == 1 { "" } else { "s" }); }
        widgets::text_center(d, b.as_str(), 436, Rgb565::CSS_GRAY);
        widgets::text_center(d, "BOOT = back", 480, Rgb565::CSS_DIM_GRAY);
    }
}
