// Spirit level: a bubble that floats to the high side, plus tilt angles.
// Uses the accelerometer (the main loop powers the IMU only while this is open).

use core::fmt::Write;

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, Line, PrimitiveStyle};

use crate::apps::widgets::{self, SBuf, CX};
use crate::apps::{App, AppInput, AppResult};

const CY: i32 = 245;
const RING_R: i32 = 150;
const BUBBLE_R: i32 = 26;
/// Degrees of tilt that move the bubble to the edge of the ring.
const FULL_SCALE_DEG: f32 = 20.0;

pub struct LevelApp {
    roll: f32,  // degrees, smoothed
    pitch: f32,
}

impl LevelApp {
    pub fn new() -> Self { Self { roll: 0.0, pitch: 0.0 } }
}

impl App for LevelApp {
    fn name(&self) -> &str { "Level" }
    fn setup(&mut self) { self.roll = 0.0; self.pitch = 0.0; }

    fn update(&mut self, input: &AppInput) -> AppResult {
        let (ax, ay, az) = input.accel;
        // Same axis mapping as the watchface gyro ball (screen x <- accel y).
        let roll = libm::atan2f(ay, libm::sqrtf(ax * ax + az * az)) * 57.29578;
        let pitch = libm::atan2f(-ax, libm::sqrtf(ay * ay + az * az)) * 57.29578;
        self.roll += (roll - self.roll) * 0.35;
        self.pitch += (pitch - self.pitch) * 0.35;
        AppResult::Continue
    }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        widgets::clear(d, Rgb565::BLACK);
        let level = self.roll.abs() < 1.0 && self.pitch.abs() < 1.0;
        widgets::title(d, if level { "LEVEL!" } else { "SPIRIT LEVEL" },
            if level { widgets::GREEN_LIGHT } else { widgets::TEAL_LIGHT });

        let ring = if level { widgets::GREEN_LIGHT } else { Rgb565::CSS_GRAY };
        let _ = Circle::with_center(Point::new(CX, CY), (RING_R * 2) as u32)
            .into_styled(PrimitiveStyle::with_stroke(ring, 3)).draw(d);
        let _ = Circle::with_center(Point::new(CX, CY), (BUBBLE_R * 2 + 12) as u32)
            .into_styled(PrimitiveStyle::with_stroke(Rgb565::CSS_DIM_GRAY, 2)).draw(d);
        let c = Rgb565::CSS_DIM_GRAY;
        let _ = Line::new(Point::new(CX - RING_R, CY), Point::new(CX + RING_R, CY))
            .into_styled(PrimitiveStyle::with_stroke(c, 1)).draw(d);
        let _ = Line::new(Point::new(CX, CY - RING_R), Point::new(CX, CY + RING_R))
            .into_styled(PrimitiveStyle::with_stroke(c, 1)).draw(d);

        // Bubble goes to the high side (opposite to a rolling ball).
        let max = (RING_R - BUBBLE_R - 4) as f32;
        let mut bx = self.roll / FULL_SCALE_DEG * max;
        let mut by = -self.pitch / FULL_SCALE_DEG * max;
        let r = libm::sqrtf(bx * bx + by * by);
        if r > max { bx *= max / r; by *= max / r; }
        let bubble = if level { widgets::GREEN_LIGHT } else { widgets::AMBER_LIGHT };
        let _ = Circle::with_center(Point::new(CX + bx as i32, CY + by as i32), (BUBBLE_R * 2) as u32)
            .into_styled(PrimitiveStyle::with_fill(bubble)).draw(d);

        let mut s: SBuf<32> = SBuf::new();
        let _ = write!(s, "X {:+.1} deg   Y {:+.1} deg", self.roll, self.pitch);
        widgets::text_center(d, s.as_str(), 435, Rgb565::WHITE);
        widgets::text_center(d, "BOOT = back", 480, Rgb565::CSS_DIM_GRAY);
    }
}
