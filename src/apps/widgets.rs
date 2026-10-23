// Small shared drawing helpers for the utility apps (Stopwatch, Timer,
// Flashlight, Steps): tappable buttons, a title line, big 7-segment numbers,
// and a tiny stack string buffer so we can use `write!` without an allocator.

use core::fmt::Write;

use embedded_graphics::mono_font::ascii::FONT_10X20;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle, RoundedRectangle};
use embedded_graphics::text::{Alignment, Text};

use crate::ui::segments;

// ---- Shared button palette ----
// Four warm/cool alternating tones, always used in this order so neighbouring
// buttons never share a colour family: teal, amber, green, coral, repeat.
// Mid-dark fills so white text stays readable; *_LIGHT tints for accents/titles.
// (No purple/fuchsia by request.)
pub const TEAL: Rgb565 = Rgb565::new(1, 31, 15);        // #0E7C7B
pub const AMBER: Rgb565 = Rgb565::new(24, 31, 1);       // #C77D0A
pub const GREEN: Rgb565 = Rgb565::new(5, 34, 10);       // #2E8B57
pub const CORAL: Rgb565 = Rgb565::new(25, 21, 7);       // #C8553D
pub const TEAL_LIGHT: Rgb565 = Rgb565::new(11, 49, 24);
pub const AMBER_LIGHT: Rgb565 = Rgb565::new(30, 45, 8);
pub const GREEN_LIGHT: Rgb565 = Rgb565::new(15, 53, 20);
pub const CORAL_LIGHT: Rgb565 = Rgb565::new(30, 35, 14);
/// Neutral for less important buttons (Reset etc.).
pub const SLATE: Rgb565 = Rgb565::new(7, 15, 8);

/// (fill, accent) for the i-th item in a list: teal, amber, green, coral, ...
pub fn palette(i: usize) -> (Rgb565, Rgb565) {
    match i % 4 {
        0 => (TEAL, TEAL_LIGHT),
        1 => (AMBER, AMBER_LIGHT),
        2 => (GREEN, GREEN_LIGHT),
        _ => (CORAL, CORAL_LIGHT),
    }
}

pub const W: i32 = 410;
pub const H: i32 = 502;
pub const CX: i32 = W / 2;

/// A rectangular tappable button. Coordinates are screen pixels.
#[derive(Clone, Copy)]
pub struct Btn {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Btn {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// True if a tap at (x, y) lands inside this button (with a small margin,
    /// since fingertips are imprecise on a 2" screen).
    pub fn hit(&self, x: u16, y: u16) -> bool {
        let (x, y) = (x as i32, y as i32);
        const SLOP: i32 = 6;
        x >= self.x - SLOP && x < self.x + self.w + SLOP
            && y >= self.y - SLOP && y < self.y + self.h + SLOP
    }

    pub fn draw<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D, label: &str, bg: Rgb565, fg: Rgb565) {
        let _ = RoundedRectangle::with_equal_corners(
            Rectangle::new(Point::new(self.x, self.y), Size::new(self.w as u32, self.h as u32)),
            Size::new(12, 12),
        )
        .into_styled(PrimitiveStyle::with_fill(bg))
        .draw(d);
        let style = MonoTextStyle::new(&FONT_10X20, fg);
        let _ = Text::with_alignment(
            label,
            Point::new(self.x + self.w / 2, self.y + self.h / 2 + 6),
            style,
            Alignment::Center,
        )
        .draw(d);
    }
}

pub fn clear<D: DrawTarget<Color = Rgb565>>(d: &mut D, color: Rgb565) {
    let _ = Rectangle::new(Point::zero(), Size::new(W as u32, H as u32))
        .into_styled(PrimitiveStyle::with_fill(color))
        .draw(d);
}

pub fn title<D: DrawTarget<Color = Rgb565>>(d: &mut D, text: &str, color: Rgb565) {
    let style = MonoTextStyle::new(&FONT_10X20, color);
    let _ = Text::with_alignment(text, Point::new(CX, 38), style, Alignment::Center).draw(d);
}

pub fn text_center<D: DrawTarget<Color = Rgb565>>(d: &mut D, text: &str, y: i32, color: Rgb565) {
    let style = MonoTextStyle::new(&FONT_10X20, color);
    let _ = Text::with_alignment(text, Point::new(CX, y), style, Alignment::Center).draw(d);
}

/// Big 7-segment number string, centered on `cx`. Supports '0'..'9', ':', '.'
/// '-' (minus) and ' ' (a blank digit-width gap). Digit size is `dw` x `dh`, stroke `t`.
pub fn big_text<D: DrawTarget<Color = Rgb565>>(
    d: &mut D, cx: i32, y: i32, s: &str, dw: i32, dh: i32, t: i32, color: Rgb565,
) {
    let gap = t;
    let narrow = t * 2 + 2; // width of ':' and '.'
    let mut total = 0;
    for c in s.bytes() {
        total += if c == b':' || c == b'.' { narrow } else { dw } + gap;
    }
    total -= gap;
    let mut x = cx - total / 2;
    for c in s.bytes() {
        match c {
            b'0'..=b'9' => {
                let _ = segments::draw_digit(d, x, y, c - b'0', dw, dh, t, color, Rgb565::BLACK);
                x += dw + gap;
            }
            b':' => {
                let _ = segments::draw_colon(d, x + narrow / 2 - t / 2, y, dh, t, color);
                x += narrow + gap;
            }
            b'.' => {
                let _ = Rectangle::new(Point::new(x + narrow / 2 - t / 2, y + dh - t), Size::new(t as u32, t as u32))
                    .into_styled(PrimitiveStyle::with_fill(color))
                    .draw(d);
                x += narrow + gap;
            }
            b'-' => {
                let _ = Rectangle::new(Point::new(x + t, y + dh / 2 - t / 2), Size::new((dw - 2 * t) as u32, t as u32))
                    .into_styled(PrimitiveStyle::with_fill(color))
                    .draw(d);
                x += dw + gap;
            }
            _ => x += dw + gap,
        }
    }
}

/// Fixed-size stack string, so apps can `write!(buf, "{:02}", n)` in no_std.
pub struct SBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> SBuf<N> {
    pub const fn new() -> Self {
        Self { buf: [0; N], len: 0 }
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> Write for SBuf<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let b = s.as_bytes();
        let n = b.len().min(N - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&b[..n]);
        self.len += n;
        Ok(())
    }
}

/// Format milliseconds as "MM:SS.t" (or "H:MM:SS" past an hour).
pub fn fmt_ms(ms: u64) -> SBuf<12> {
    let mut b = SBuf::new();
    let total_s = ms / 1000;
    let h = total_s / 3600;
    let m = (total_s / 60) % 60;
    let s = total_s % 60;
    if h > 0 {
        let _ = write!(b, "{}:{:02}:{:02}", h, m, s);
    } else {
        let _ = write!(b, "{:02}:{:02}.{}", m, s, (ms / 100) % 10);
    }
    b
}
