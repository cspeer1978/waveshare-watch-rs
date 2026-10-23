// Flashlight: fills the screen with a solid color at a fixed panel brightness.
// Tap anywhere to cycle: bright white -> soft white -> red (night vision).
// The main loop applies `brightness()` to the panel, keeps the screen awake
// for KEEP_AWAKE_SECS after the last tap, and restores the user's normal
// brightness on exit.

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;

use crate::apps::widgets;
use crate::apps::{App, AppInput, AppResult};

/// How long the light stays on after the last tap before normal sleep resumes.
pub const KEEP_AWAKE_SECS: u64 = 120;

const MODES: [(Rgb565, u8, &str); 3] = [
    (Rgb565::WHITE, 0xFF, "BRIGHT"),
    (Rgb565::WHITE, 0x60, "SOFT"),
    (Rgb565::RED, 0xC0, "NIGHT"),
];

pub struct FlashlightApp {
    mode: usize,
}

impl FlashlightApp {
    pub fn new() -> Self {
        Self { mode: 0 }
    }

    pub fn handle_tap(&mut self) {
        self.mode = (self.mode + 1) % MODES.len();
    }

    /// Panel brightness for the current mode (0..255).
    pub fn brightness(&self) -> u8 {
        MODES[self.mode].1
    }
}

impl App for FlashlightApp {
    fn name(&self) -> &str { "Flashlight" }

    fn setup(&mut self) {
        self.mode = 0;
    }

    fn update(&mut self, _input: &AppInput) -> AppResult {
        AppResult::Continue
    }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        let (color, _, label) = MODES[self.mode];
        widgets::clear(d, color);
        let hint = Rgb565::new(5, 10, 5); // dark grey: readable on white and red
        widgets::text_center(d, label, 440, hint);
        widgets::text_center(d, "tap = mode   BOOT = back", 470, hint);
    }
}
