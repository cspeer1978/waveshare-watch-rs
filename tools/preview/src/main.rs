extern crate alloc;
pub mod ui {
    #[path = "../../../../src/ui/power_page.rs"] pub mod power_page;
    #[path = "../../../../src/ui/segments.rs"] pub mod segments;
    #[path = "../../../../src/ui/launcher.rs"] pub mod launcher;
    #[path = "../../../../src/ui/bigclock.rs"] pub mod bigclock;
    #[path = "../../../../src/ui/watchface.rs"] pub mod watchface;
    #[path = "../../../../src/ui/pages.rs"] pub mod pages;
}
pub mod drivers { pub mod co5300 { #[derive(Debug)] pub enum DisplayError { X } } }
pub mod board { pub const LCD_WIDTH: u16 = 410; pub const LCD_HEIGHT: u16 = 502; }
pub mod peripherals {
    #[path = "../../../../src/peripherals/power_stats.rs"] pub mod power_stats;
    #[path = "../../../../src/peripherals/battlog.rs"] pub mod battlog;
    pub mod touch {
        #[derive(Clone, Copy, PartialEq, Debug)] pub enum SwipeDirection { Up, Down, Left, Right, Tap }
        #[derive(Clone, Copy, Debug)] pub struct TouchPoint { pub x: u16, pub y: u16, pub fingers: u8 }
    }
}
pub mod apps {
    use embedded_graphics::pixelcolor::Rgb565;
    use embedded_graphics::prelude::DrawTarget;
    use crate::peripherals::touch::{SwipeDirection, TouchPoint};
    #[path = "../../../../src/apps/widgets.rs"] pub mod widgets;
    #[path = "../../../../src/apps/stopwatch.rs"] pub mod stopwatch;
    #[path = "../../../../src/apps/timer.rs"] pub mod timer;
    #[path = "../../../../src/apps/steps.rs"] pub mod steps;
    #[path = "../../../../src/apps/flashlight.rs"] pub mod flashlight;
    #[path = "../../../../src/apps/smarthome.rs"] pub mod smarthome;
    #[path = "../../../../src/apps/ha_info.rs"] pub mod ha_info;
    #[path = "../../../../src/apps/alarm.rs"] pub mod alarm;
    #[path = "../../../../src/apps/calculator.rs"] pub mod calculator;
    #[path = "../../../../src/apps/level.rs"] pub mod level;
    #[path = "../../../../src/apps/dice.rs"] pub mod dice;
    #[path = "../../../../src/apps/sunmoon.rs"] pub mod sunmoon;
    #[path = "../../../../src/apps/tictactoe.rs"] pub mod tictactoe;
    #[path = "../../../../src/apps/asteroids.rs"] pub mod asteroids;
    #[path = "../../../../src/apps/wifiqr.rs"] pub mod wifiqr;
    #[path = "../../../../src/apps/fluid.rs"] pub mod fluid;
    #[path = "../../../../src/apps/waves.rs"] pub mod waves;
    #[path = "../../../../src/apps/battery.rs"] pub mod battery;
    #[path = "../../../../src/apps/settings.rs"] pub mod settings;
    #[path = "../../../../src/apps/tetris.rs"] pub mod tetris;
    #[path = "../../../../src/apps/maze.rs"] pub mod maze;
    pub struct AppInput { pub touch: Option<TouchPoint>, pub swipe: Option<SwipeDirection>, pub tap: bool, pub accel: (f32,f32,f32), pub dt_ms: u32 }
    pub enum AppResult { Continue, Exit }
    pub trait App { fn name(&self) -> &str; fn setup(&mut self); fn update(&mut self, input: &AppInput) -> AppResult; fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D); }
    #[derive(Clone, Copy, PartialEq, Debug)]
    pub enum AppState { Watchface, Launcher, Tetris, Maze, Settings, SmartHome, Stopwatch, Timer, Flashlight, Steps, HaStatus, Weather, Fleet, Alarm, Calculator, Level, Dice, SunMoon, TicTacToe, Asteroids, WifiQr, Fluid, Waves, BallPit, Battery }
}
mod fb;
fn main() { fb::run(); }
