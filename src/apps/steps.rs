// Step counter. Counts only while this app is open and counting is on —
// nothing runs in the background once you leave the app.
//
// Battery notes (the main loop does the hardware side):
//   * Only the accelerometer is powered (gyro stays off — it's the hungry part).
//   * Sampled at ~20 Hz, which is plenty for walking cadence (~2 steps/s).
//   * Counting keeps going with the screen off, but auto-pauses after
//     AUTO_PAUSE_SECS with no steps, so a forgotten app doesn't drain the battery.
//
// Detection: squared acceleration magnitude (no sqrt needed in no_std), with a
// fast filter and a slow baseline (~1 s). A step is when fast rises HIGH above
// the baseline after having dropped back below LOW, at least MIN_GAP apart.
// Baseline-relative so it works regardless of how the wrist is tilted.
// Tested on simulated signals: walking/running counted exactly, and 0 steps
// for standing still or occasional single arm movements.
// To ignore one-off arm gestures, a step only counts once two candidates arrive
// within MAX_GAP of each other (the first one is then counted retroactively).

use core::fmt::Write;

use embassy_time::Instant;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;

use crate::apps::widgets::{self, Btn, SBuf, CX};
use crate::apps::{App, AppInput, AppResult};

pub const AUTO_PAUSE_SECS: u64 = 5 * 60;
const HIGH: f32 = 0.20; // g² above baseline = peak (raise if it over-counts)
const LOW: f32 = 0.02;  // g² above baseline = re-armed
const MIN_GAP_MS: u64 = 280;
const MAX_GAP_MS: u64 = 2000;

const START_BTN: Btn = Btn::new(30, 300, 165, 70);
const RESET_BTN: Btn = Btn::new(215, 300, 165, 70);

pub struct StepsApp {
    steps: u32,
    counting: bool,
    fast: f32,
    base: f32,
    above: bool,
    streak: u32,
    last_candidate: Option<Instant>,
    last_step: Instant,
    auto_paused: bool,
}

impl StepsApp {
    pub fn new() -> Self {
        Self {
            steps: 0,
            counting: false,
            fast: 1.0,
            base: 1.0,
            above: false,
            streak: 0,
            last_candidate: None,
            last_step: Instant::now(),
            auto_paused: false,
        }
    }

    pub fn is_counting(&self) -> bool {
        self.counting
    }

    pub fn steps(&self) -> u32 {
        self.steps
    }

    /// Feed one accelerometer sample (in g). Returns true if the count changed.
    pub fn feed(&mut self, x: f32, y: f32, z: f32) -> bool {
        if !self.counting {
            return false;
        }
        let now = Instant::now();
        if (now - self.last_step).as_secs() >= AUTO_PAUSE_SECS {
            self.counting = false;
            self.auto_paused = true;
            return true; // redraw to show "paused"
        }

        let m2 = x * x + y * y + z * z;
        self.fast = self.fast * 0.5 + m2 * 0.5;
        self.base = self.base * 0.95 + m2 * 0.05;
        let dev = self.fast - self.base;

        if self.above {
            if dev < LOW {
                self.above = false;
            }
            return false;
        }
        if dev <= HIGH {
            return false;
        }
        // Rising edge above HIGH = candidate step.
        let gap = self.last_candidate.map(|t| (now - t).as_millis());
        if let Some(g) = gap {
            if g < MIN_GAP_MS {
                return false; // too soon: same bounce
            }
        }
        self.above = true;
        self.last_candidate = Some(now);
        match gap {
            Some(g) if g <= MAX_GAP_MS => {
                // Rhythmic: count it (and the first one of the streak).
                self.steps += if self.streak == 1 { 2 } else { 1 };
                self.streak += 1;
                self.last_step = now;
                true
            }
            _ => {
                self.streak = 1; // first of a possible streak, not counted yet
                false
            }
        }
    }

    /// Returns true if the tap changed something (caller should redraw).
    pub fn handle_tap(&mut self, x: u16, y: u16) -> bool {
        if START_BTN.hit(x, y) {
            self.counting = !self.counting;
            if self.counting {
                self.last_step = Instant::now();
                self.auto_paused = false;
                self.streak = 0;
                self.last_candidate = None;
                self.above = false;
            }
            return true;
        }
        if RESET_BTN.hit(x, y) {
            let counting = self.counting;
            *self = Self::new();
            self.counting = counting;
            return true;
        }
        false
    }

    /// Stop counting (called when leaving the app, so the sensor powers down).
    pub fn stop(&mut self) {
        self.counting = false;
    }
}

impl App for StepsApp {
    fn name(&self) -> &str { "Steps" }

    // Opening the app starts counting; the total is kept until RESET.
    fn setup(&mut self) {
        self.counting = true;
        self.last_step = Instant::now();
        self.auto_paused = false;
        self.streak = 0;
        self.last_candidate = None;
        self.above = false;
    }

    fn update(&mut self, _input: &AppInput) -> AppResult {
        AppResult::Continue
    }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        widgets::clear(d, Rgb565::BLACK);
        widgets::title(d, "STEPS", widgets::GREEN_LIGHT);

        let mut n: SBuf<8> = SBuf::new();
        let _ = write!(n, "{}", self.steps);
        widgets::big_text(d, CX, 90, n.as_str(), 56, 100, 8, Rgb565::WHITE);

        let status = if self.counting {
            "counting..."
        } else if self.auto_paused {
            "paused (no steps 5 min)"
        } else {
            "paused"
        };
        let sc = if self.counting { Rgb565::GREEN } else { Rgb565::YELLOW };
        widgets::text_center(d, status, 250, sc);

        let (label, bg) = if self.counting {
            ("PAUSE", widgets::CORAL)
        } else {
            ("START", widgets::GREEN)
        };
        START_BTN.draw(d, label, bg, Rgb565::WHITE);
        RESET_BTN.draw(d, "RESET", widgets::SLATE, Rgb565::WHITE);

        widgets::text_center(d, "Counts only while open", 420, Rgb565::CSS_GRAY);
        widgets::text_center(d, "BOOT = back", 480, Rgb565::CSS_DIM_GRAY);
    }
}
