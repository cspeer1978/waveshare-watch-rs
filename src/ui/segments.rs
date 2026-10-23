// 7-segment style digit renderer using fill_solid rectangles
// Generic over DrawTarget - works with framebuffer or direct display

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};

const SEGMENTS: [u8; 10] = [
    0b0111111, 0b0000110, 0b1011011, 0b1001111, 0b1100110,
    0b1101101, 0b1111101, 0b0000111, 0b1111111, 0b1101111,
];

pub fn draw_digit<D: DrawTarget<Color = Rgb565>>(
    display: &mut D, x: i32, y: i32, digit: u8,
    w: i32, h: i32, t: i32, color: Rgb565, bg: Rgb565,
) -> Result<(), D::Error> {
    if digit > 9 { return Ok(()); }
    let seg = SEGMENTS[digit as usize];
    let hh = h / 2;
    let on = |bit: u8| -> Rgb565 { if seg & bit != 0 { color } else { bg } };

    // A: top, B: top-right, C: bottom-right, D: bottom, E: bottom-left, F: top-left, G: middle
    seg_h(display, x + t, y, w - 2*t, t, on(0x01))?;
    seg_v(display, x + w - t, y + t, hh - t, t, on(0x02))?;
    seg_v(display, x + w - t, y + hh + t, hh - t, t, on(0x04))?;
    seg_h(display, x + t, y + h - t, w - 2*t, t, on(0x08))?;
    seg_v(display, x, y + hh + t, hh - t, t, on(0x10))?;
    seg_v(display, x, y + t, hh - t, t, on(0x20))?;
    seg_h(display, x + t, y + hh - t/2, w - 2*t, t, on(0x40))
}

fn seg_h<D: DrawTarget<Color = Rgb565>>(d: &mut D, x: i32, y: i32, w: i32, h: i32, c: Rgb565) -> Result<(), D::Error> {
    Rectangle::new(Point::new(x, y), Size::new(w as u32, h as u32))
        .into_styled(PrimitiveStyle::with_fill(c)).draw(d).map(|_| ())
}

fn seg_v<D: DrawTarget<Color = Rgb565>>(d: &mut D, x: i32, y: i32, h: i32, w: i32, c: Rgb565) -> Result<(), D::Error> {
    Rectangle::new(Point::new(x, y), Size::new(w as u32, h as u32))
        .into_styled(PrimitiveStyle::with_fill(c)).draw(d).map(|_| ())
}

pub fn draw_colon<D: DrawTarget<Color = Rgb565>>(d: &mut D, x: i32, y: i32, h: i32, t: i32, color: Rgb565) -> Result<(), D::Error> {
    let s = t as u32;
    Rectangle::new(Point::new(x, y + h/4), Size::new(s, s))
        .into_styled(PrimitiveStyle::with_fill(color)).draw(d).map(|_| ())?;
    Rectangle::new(Point::new(x, y + 3*h/4 - t), Size::new(s, s))
        .into_styled(PrimitiveStyle::with_fill(color)).draw(d).map(|_| ())
}

/// Lay out a row of glyphs (Some(digit) = digit, None = colon) and return the
/// starting x so the *lit pixels* are centered on `center_x`. A blank leading
/// hour digit draws nothing, and a 7-segment "1" only lights its right stroke,
/// so centering on the digit boxes looks shifted right; this measures the ink.
fn ink_centered_x(center_x: i32, glyphs: &[Option<u8>], skip_first: bool,
                  dw: i32, t: i32, gap: i32, cw: i32) -> i32 {
    let mut x = 0;
    let mut ink_l = i32::MAX;
    let mut ink_r = 0;
    for (i, g) in glyphs.iter().enumerate() {
        match g {
            Some(digit) => {
                if !(i == 0 && skip_first) {
                    let l = if *digit == 1 { x + dw - t } else { x };
                    ink_l = ink_l.min(l);
                    ink_r = ink_r.max(x + dw);
                }
                x += dw + gap;
            }
            None => {
                let cx = x + cw / 2 - t / 2;
                ink_l = ink_l.min(cx);
                ink_r = ink_r.max(cx + t);
                x += cw + gap;
            }
        }
    }
    center_x - (ink_l + ink_r) / 2
}

pub fn draw_time<D: DrawTarget<Color = Rgb565>>(
    d: &mut D, center_x: i32, y: i32,
    h: u8, m: u8, s: u8, color: Rgb565, bg: Rgb565,
) -> Result<(), D::Error> {
    let dw = 36; let dh = 64; let t = 6; let gap = 6; let cw = 14;
    let glyphs = [Some(h/10), Some(h%10), None, Some(m/10), Some(m%10), None, Some(s/10), Some(s%10)];
    // 12-hour display: a leading zero on the hour is left blank ("9:05", not "09:05").
    let mut x = ink_centered_x(center_x, &glyphs, h < 10, dw, t, gap, cw);

    if h >= 10 { draw_digit(d, x, y, h/10, dw, dh, t, color, bg)?; }
    x += dw+gap;
    draw_digit(d, x, y, h%10, dw, dh, t, color, bg)?; x += dw+gap;
    draw_colon(d, x+cw/2-t/2, y, dh, t, color)?; x += cw+gap;
    draw_digit(d, x, y, m/10, dw, dh, t, color, bg)?; x += dw+gap;
    draw_digit(d, x, y, m%10, dw, dh, t, color, bg)?; x += dw+gap;
    draw_colon(d, x+cw/2-t/2, y, dh, t, color)?; x += cw+gap;
    draw_digit(d, x, y, s/10, dw, dh, t, color, bg)?; x += dw+gap;
    draw_digit(d, x, y, s%10, dw, dh, t, color, bg)
}

/// Draw HH:MM only, larger digits, used by the Always-On-Display.
pub fn draw_hhmm<D: DrawTarget<Color = Rgb565>>(
    d: &mut D, center_x: i32, y: i32,
    h: u8, m: u8, color: Rgb565, bg: Rgb565,
) -> Result<(), D::Error> {
    let _ = bg;
    // Bigger than the normal watchface — fewer chars so we have horizontal room.
    draw_hhmm_sized(d, center_x, y, h, m, 56, 96, 8, 8, 18, color)
}

/// HH:MM at any size, centered on the lit pixels (blank leading hour digit,
/// narrow "1"). Used by the AOD, Big Clock page and Alarm app.
pub fn draw_hhmm_sized<D: DrawTarget<Color = Rgb565>>(
    d: &mut D, center_x: i32, y: i32, h: u8, m: u8,
    dw: i32, dh: i32, t: i32, gap: i32, cw: i32, color: Rgb565,
) -> Result<(), D::Error> {
    let bg = Rgb565::BLACK;
    let glyphs = [Some(h/10), Some(h%10), None, Some(m/10), Some(m%10)];
    let mut x = ink_centered_x(center_x, &glyphs, h < 10, dw, t, gap, cw);
    // Leading zero on the hour left blank ("9:05").
    if h >= 10 { draw_digit(d, x, y, h/10, dw, dh, t, color, bg)?; }
    x += dw+gap;
    draw_digit(d, x, y, h%10, dw, dh, t, color, bg)?; x += dw+gap;
    draw_colon(d, x+cw/2-t/2, y, dh, t, color)?; x += cw+gap;
    draw_digit(d, x, y, m/10, dw, dh, t, color, bg)?; x += dw+gap;
    draw_digit(d, x, y, m%10, dw, dh, t, color, bg)
}

/// Convert a 24-hour value (0..23) to 12-hour (1..12).
pub fn to_12h(h: u8) -> u8 {
    match h % 12 { 0 => 12, v => v }
}
