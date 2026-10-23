// Big Clock page: huge HH:MM, AM/PM, weekday + date, battery.
// Lives in the watchface swipe pages right after the normal Clock page.
// Only redrawn when the minute changes, so it's as cheap as the main face.

use core::fmt::Write;

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;

use crate::apps::widgets::{self, SBuf, CX};
use crate::ui::segments;

const DAYS: [&str; 7] = ["SUNDAY", "MONDAY", "TUESDAY", "WEDNESDAY", "THURSDAY", "FRIDAY", "SATURDAY"];
const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];

/// h24/m from the RTC; year is 0..99 (20YY).
pub fn draw_big_clock<D: DrawTarget<Color = Rgb565>>(
    d: &mut D, h24: u8, m: u8, day: u8, month: u8, year: u8, batt_pct: u8, charging: bool,
) {
    widgets::clear(d, Rgb565::BLACK);
    let wd = crate::apps::sunmoon::weekday(2000 + year as i32, month as u32, day as u32) as usize;
    widgets::text_center(d, DAYS[wd % 7], 70, widgets::TEAL_LIGHT);

    let _ = segments::draw_hhmm_sized(d, CX, 110, segments::to_12h(h24), m, 72, 140, 13, 12, 26, Rgb565::WHITE);
    widgets::text_center(d, if h24 >= 12 { "PM" } else { "AM" }, 288, widgets::AMBER_LIGHT);

    let mut s: SBuf<24> = SBuf::new();
    let mi = (month.clamp(1, 12) - 1) as usize;
    let _ = write!(s, "{} {}, 20{:02}", MONTHS[mi], day, year);
    widgets::text_center(d, s.as_str(), 330, Rgb565::WHITE);

    let mut b: SBuf<16> = SBuf::new();
    let _ = write!(b, "{}%{}", batt_pct, if charging { " CHG" } else { "" });
    let bc = if batt_pct <= 15 { widgets::CORAL_LIGHT } else { Rgb565::CSS_GRAY };
    widgets::text_center(d, b.as_str(), 400, bc);
}
