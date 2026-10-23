// Dice roller: shake the watch (or tap the dice) to roll. 1 or 2 dice.

use core::fmt::Write;

use embassy_time::Instant;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, PrimitiveStyle, Rectangle, RoundedRectangle};

use crate::apps::widgets::{self, Btn, SBuf};
use crate::apps::{App, AppInput, AppResult};

const COUNT_BTN: Btn = Btn::new(110, 395, 190, 55);
const ROLL_FRAMES: u8 = 10;
/// Squared acceleration magnitude that counts as a shake (~1.7 g).
const SHAKE_G2: f32 = 2.9;

pub struct DiceApp {
    count: u8,
    values: [u8; 2],
    rolling: u8, // frames of tumbling animation left
    rng: u32,
    cooldown: u8,
}

impl DiceApp {
    pub fn new() -> Self {
        Self { count: 2, values: [6, 6], rolling: 0, rng: 0x1234_5678, cooldown: 0 }
    }

    fn rand6(&mut self) -> u8 {
        // xorshift32
        let mut x = self.rng;
        x ^= x << 13; x ^= x >> 17; x ^= x << 5;
        self.rng = x;
        (x % 6) as u8 + 1
    }

    fn roll(&mut self) {
        self.rng ^= Instant::now().as_ticks() as u32 | 1;
        self.rolling = ROLL_FRAMES;
    }

    pub fn is_rolling(&self) -> bool { self.rolling > 0 }

    /// Returns true if something changed.
    pub fn handle_tap(&mut self, x: u16, y: u16) -> bool {
        if COUNT_BTN.hit(x, y) {
            self.count = if self.count == 1 { 2 } else { 1 };
            return true;
        }
        if y > 60 && y < 380 { self.roll(); return true; }
        false
    }

    fn draw_die<D: DrawTarget<Color = Rgb565>>(d: &mut D, cx: i32, cy: i32, size: i32, v: u8, face: Rgb565) {
        let _ = RoundedRectangle::with_equal_corners(
            Rectangle::new(Point::new(cx - size / 2, cy - size / 2), Size::new(size as u32, size as u32)),
            Size::new(size as u32 / 6, size as u32 / 6),
        ).into_styled(PrimitiveStyle::with_fill(face)).draw(d);
        let o = size * 27 / 100;
        let pr = (size / 9) as u32 * 2;
        let pip = |d: &mut D, dx: i32, dy: i32| {
            let _ = Circle::with_center(Point::new(cx + dx, cy + dy), pr)
                .into_styled(PrimitiveStyle::with_fill(Rgb565::new(3, 6, 3))).draw(d);
        };
        if v % 2 == 1 { pip(d, 0, 0); }
        if v >= 2 { pip(d, -o, -o); pip(d, o, o); }
        if v >= 4 { pip(d, o, -o); pip(d, -o, o); }
        if v == 6 { pip(d, -o, 0); pip(d, o, 0); }
    }
}

impl App for DiceApp {
    fn name(&self) -> &str { "Dice" }
    fn setup(&mut self) { self.rolling = 0; self.cooldown = 10; }

    fn update(&mut self, input: &AppInput) -> AppResult {
        let (ax, ay, az) = input.accel;
        if self.cooldown > 0 { self.cooldown -= 1; }
        if self.rolling == 0 && self.cooldown == 0 && ax * ax + ay * ay + az * az > SHAKE_G2 {
            self.roll();
        }
        if self.rolling > 0 {
            self.rolling -= 1;
            self.values[0] = self.rand6();
            self.values[1] = self.rand6();
            if self.rolling == 0 { self.cooldown = 12; }
        }
        AppResult::Continue
    }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        widgets::clear(d, Rgb565::BLACK);
        widgets::title(d, "DICE", widgets::CORAL_LIGHT);
        let face = if self.rolling > 0 { Rgb565::new(24, 48, 22) } else { Rgb565::new(31, 61, 28) };
        // Slight wobble while tumbling
        let wob = if self.rolling > 0 { (self.rolling as i32 % 3 - 1) * 6 } else { 0 };
        if self.count == 1 {
            Self::draw_die(d, 205 + wob, 215 - wob, 170, self.values[0], face);
        } else {
            Self::draw_die(d, 115 + wob, 215, 150, self.values[0], face);
            Self::draw_die(d, 295 - wob, 215 + wob, 150, self.values[1], face);
        }
        let mut s: SBuf<24> = SBuf::new();
        if self.rolling > 0 {
            let _ = write!(s, "Rolling...");
        } else if self.count == 2 {
            let _ = write!(s, "Total {}", self.values[0] + self.values[1]);
        } else {
            let _ = write!(s, "Shake or tap to roll");
        }
        widgets::text_center(d, s.as_str(), 345, Rgb565::WHITE);
        COUNT_BTN.draw(d, if self.count == 1 { "1 DIE" } else { "2 DICE" }, widgets::TEAL, Rgb565::WHITE);
        widgets::text_center(d, "BOOT = back", 480, Rgb565::CSS_DIM_GRAY);
    }
}
