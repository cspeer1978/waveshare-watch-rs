// Countdown timer. Set with -1m / +1m / +10s, START/PAUSE, RESET.
// While running it stores an absolute end Instant, so it keeps time with the
// screen off. The main loop shortens its sleep so it wakes right at the end,
// then wakes the screen and beeps (see check_expired / remaining_ms).

use embassy_time::{Duration, Instant};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;

use crate::apps::widgets::{self, Btn, SBuf, CX};
use crate::apps::{App, AppInput, AppResult};
use core::fmt::Write;

const MINUS_BTN: Btn = Btn::new(30, 200, 105, 60);
const PLUS_BTN: Btn = Btn::new(152, 200, 105, 60);
const PLUS10_BTN: Btn = Btn::new(275, 200, 105, 60);
const START_BTN: Btn = Btn::new(30, 285, 165, 70);
const RESET_BTN: Btn = Btn::new(215, 285, 165, 70);

const DEFAULT_SECS: u32 = 5 * 60;
const MAX_SECS: u32 = 99 * 60 + 59;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    Running,
    Paused,
    Done,
}

pub struct TimerApp {
    set_secs: u32,
    state: State,
    end_at: Option<Instant>,
    paused_left_ms: u64,
}

impl TimerApp {
    pub fn new() -> Self {
        Self { set_secs: DEFAULT_SECS, state: State::Idle, end_at: None, paused_left_ms: 0 }
    }

    /// Milliseconds left while running; None if not running.
    /// The main loop uses this to cap its sleep so the alarm fires on time.
    pub fn remaining_ms(&self) -> Option<u64> {
        match (self.state, self.end_at) {
            (State::Running, Some(end)) => {
                let now = Instant::now();
                Some(if end > now { (end - now).as_millis() } else { 0 })
            }
            _ => None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.state == State::Running
    }

    /// Call every loop iteration. Returns true exactly once, when the timer hits zero.
    pub fn check_expired(&mut self) -> bool {
        if self.remaining_ms() == Some(0) {
            self.state = State::Done;
            self.end_at = None;
            return true;
        }
        false
    }

    fn left_ms(&self) -> u64 {
        match self.state {
            State::Running => self.remaining_ms().unwrap_or(0),
            State::Paused => self.paused_left_ms,
            State::Done => 0,
            State::Idle => self.set_secs as u64 * 1000,
        }
    }

    fn adjust(&mut self, delta: i32) {
        if self.state == State::Done {
            self.state = State::Idle;
        }
        if self.state != State::Idle {
            return;
        }
        let v = (self.set_secs as i32 + delta).clamp(10, MAX_SECS as i32);
        self.set_secs = v as u32;
    }

    /// Returns true if the tap changed something (caller should redraw).
    pub fn handle_tap(&mut self, x: u16, y: u16) -> bool {
        if MINUS_BTN.hit(x, y) { self.adjust(-60); return true; }
        if PLUS_BTN.hit(x, y) { self.adjust(60); return true; }
        if PLUS10_BTN.hit(x, y) { self.adjust(10); return true; }
        if START_BTN.hit(x, y) {
            match self.state {
                State::Idle | State::Done => {
                    self.end_at = Some(Instant::now() + Duration::from_secs(self.set_secs as u64));
                    self.state = State::Running;
                }
                State::Running => {
                    self.paused_left_ms = self.remaining_ms().unwrap_or(0);
                    self.end_at = None;
                    self.state = State::Paused;
                }
                State::Paused => {
                    self.end_at = Some(Instant::now() + Duration::from_millis(self.paused_left_ms));
                    self.state = State::Running;
                }
            }
            return true;
        }
        if RESET_BTN.hit(x, y) {
            self.state = State::Idle;
            self.end_at = None;
            return true;
        }
        false
    }
}

impl App for TimerApp {
    fn name(&self) -> &str { "Timer" }

    // Keep a running timer when leaving/re-entering the app.
    fn setup(&mut self) {}

    fn update(&mut self, _input: &AppInput) -> AppResult {
        AppResult::Continue
    }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        widgets::clear(d, Rgb565::BLACK);
        widgets::title(d, "TIMER", widgets::AMBER_LIGHT);

        // Round up so "00:01" shows during the last second, and 00:00 only at the end.
        let left_s = (self.left_ms() + 999) / 1000;
        let mut t: SBuf<8> = SBuf::new();
        let _ = write!(t, "{:02}:{:02}", left_s / 60, left_s % 60);
        let color = match self.state {
            State::Done => Rgb565::RED,
            State::Running => Rgb565::WHITE,
            _ => Rgb565::CSS_LIGHT_GRAY,
        };
        widgets::big_text(d, CX, 70, t.as_str(), 56, 96, 8, color);

        let set_bg = if self.state == State::Idle || self.state == State::Done {
            widgets::AMBER
        } else {
            Rgb565::new(5, 8, 3) // greyed out while running/paused
        };
        MINUS_BTN.draw(d, "-1m", set_bg, Rgb565::WHITE);
        PLUS_BTN.draw(d, "+1m", set_bg, Rgb565::WHITE);
        PLUS10_BTN.draw(d, "+10s", set_bg, Rgb565::WHITE);

        let (label, bg) = match self.state {
            State::Running => ("PAUSE", widgets::CORAL),
            State::Paused => ("RESUME", widgets::GREEN),
            _ => ("START", widgets::GREEN),
        };
        START_BTN.draw(d, label, bg, Rgb565::WHITE);
        RESET_BTN.draw(d, "RESET", widgets::SLATE, Rgb565::WHITE);

        if self.state == State::Done {
            widgets::text_center(d, "TIME'S UP!", 400, Rgb565::RED);
        }
        widgets::text_center(d, "BOOT = back", 480, Rgb565::CSS_DIM_GRAY);
    }
}
