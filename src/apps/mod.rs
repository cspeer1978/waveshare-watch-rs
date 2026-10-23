// App framework - common types and trait for all apps/games

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::DrawTarget;

use crate::peripherals::touch::{SwipeDirection, TouchPoint};

pub mod tetris;
pub mod maze;
pub mod settings;
pub mod smarthome;
pub mod widgets;
pub mod stopwatch;
pub mod timer;
pub mod flashlight;
pub mod steps;
pub mod ha_info;
pub mod alarm;
pub mod calculator;
pub mod level;
pub mod dice;
pub mod sunmoon;
pub mod tictactoe;
pub mod asteroids;
pub mod wifiqr;
pub mod fluid;
pub mod waves;
pub mod battery;

/// Input state passed to apps each frame
pub struct AppInput {
    pub touch: Option<TouchPoint>,
    pub swipe: Option<SwipeDirection>,
    pub tap: bool,
    pub accel: (f32, f32, f32),
    pub dt_ms: u32, // milliseconds since last frame
}

/// Result of an app update
pub enum AppResult {
    Continue,
    Exit, // Return to launcher/watchface
}

/// Common trait for all apps/games
pub trait App {
    fn name(&self) -> &str;
    fn setup(&mut self);
    fn update(&mut self, input: &AppInput) -> AppResult;
    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D);
}

/// All available app states
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AppState {
    Watchface,
    Launcher,
    Tetris,
    Maze,
    SmartHome,
    Stopwatch,
    Timer,
    Flashlight,
    Steps,
    HaStatus,
    Weather,
    Fleet,
    Alarm,
    Calculator,
    Level,
    Dice,
    SunMoon,
    TicTacToe,
    Asteroids,
    WifiQr,
    Fluid,
    Waves,
    Battery,
    Settings,
}
