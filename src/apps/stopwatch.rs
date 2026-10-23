// Stopwatch: START/STOP plus LAP (while running) or RESET (while stopped).
// Time comes from the monotonic embassy clock, so it keeps counting
// correctly while the screen is off — nothing has to tick in the background.

use core::fmt::Write;

use embassy_time::Instant;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;

use crate::apps::widgets::{self, Btn, SBuf, CX};
use crate::apps::{App, AppInput, AppResult};

const START_BTN: Btn = Btn::new(30, 210, 165, 70);
const LAP_BTN: Btn = Btn::new(215, 210, 165, 70);
const MAX_LAPS: usize = 4;

pub struct StopwatchApp {
    running: bool,
    started_at: Option<Instant>,
    accumulated_ms: u64,
    laps: [u64; MAX_LAPS], // newest first
    lap_count: u32,
    last_lap_total: u64,
}

impl StopwatchApp {
    pub fn new() -> Self {
        Self {
            running: false,
            started_at: None,
            accumulated_ms: 0,
            laps: [0; MAX_LAPS],
            lap_count: 0,
            last_lap_total: 0,
        }
    }

    pub fn elapsed_ms(&self) -> u64 {
        let live = match (self.running, self.started_at) {
            (true, Some(t)) => (Instant::now() - t).as_millis(),
            _ => 0,
        };
        self.accumulated_ms + live
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Returns true if the tap changed something (caller should redraw).
    pub fn handle_tap(&mut self, x: u16, y: u16) -> bool {
        if START_BTN.hit(x, y) {
            if self.running {
                self.accumulated_ms = self.elapsed_ms();
                self.running = false;
                self.started_at = None;
            } else {
                self.running = true;
                self.started_at = Some(Instant::now());
            }
            return true;
        }
        if LAP_BTN.hit(x, y) {
            if self.running {
                let total = self.elapsed_ms();
                let lap = total - self.last_lap_total;
                self.last_lap_total = total;
                for i in (1..MAX_LAPS).rev() {
                    self.laps[i] = self.laps[i - 1];
                }
                self.laps[0] = lap;
                self.lap_count += 1;
            } else {
                *self = Self::new();
            }
            return true;
        }
        false
    }
}

impl App for StopwatchApp {
    fn name(&self) -> &str { "Stopwatch" }

    // Keep the running state when leaving/re-entering the app, like a real stopwatch.
    fn setup(&mut self) {}

    fn update(&mut self, _input: &AppInput) -> AppResult {
        AppResult::Continue
    }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        widgets::clear(d, Rgb565::BLACK);
        widgets::title(d, "STOPWATCH", widgets::TEAL_LIGHT);

        let t = widgets::fmt_ms(self.elapsed_ms());
        let color = if self.running { Rgb565::WHITE } else { Rgb565::CSS_LIGHT_GRAY };
        widgets::big_text(d, CX, 80, t.as_str(), 44, 80, 7, color);

        let (label, bg) = if self.running {
            ("STOP", widgets::CORAL)
        } else if self.elapsed_ms() > 0 {
            ("RESUME", widgets::GREEN)
        } else {
            ("START", widgets::GREEN)
        };
        START_BTN.draw(d, label, bg, Rgb565::WHITE);
        let (lap_label, lap_bg) = if self.running { ("LAP", widgets::TEAL) } else { ("RESET", widgets::SLATE) };
        LAP_BTN.draw(d, lap_label, lap_bg, Rgb565::WHITE);

        // Lap list, newest first.
        let shown = (self.lap_count as usize).min(MAX_LAPS);
        for i in 0..shown {
            let mut line: SBuf<24> = SBuf::new();
            let n = self.lap_count as usize - i;
            let _ = write!(line, "Lap {:>2}  {}", n, widgets::fmt_ms(self.laps[i]).as_str());
            let c = if i == 0 { Rgb565::YELLOW } else { Rgb565::CSS_GRAY };
            widgets::text_center(d, line.as_str(), 320 + i as i32 * 32, c);
        }

        widgets::text_center(d, "BOOT = back", 480, Rgb565::CSS_DIM_GRAY);
    }
}
