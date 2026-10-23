// App Launcher - scrollable list of apps
// Drag to scroll (follows the finger, flicks glide), tap to open, swipe right to go back

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle, RoundedRectangle};
use embedded_graphics::mono_font::ascii::FONT_10X20;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::text::{Alignment, Text};

use crate::apps::AppState;
use crate::peripherals::touch::SwipeDirection;

const ITEM_H: i32 = 65;
const ITEM_GAP: i32 = 6;
const MARGIN_X: i32 = 20;
const START_Y: i32 = 55;
const SCREEN_W: i32 = 410;

struct MenuItem {
    name: &'static str,
    state: AppState,
}

// Colours come from the shared palette by position (teal, amber, green, coral,
// repeat), so the list always alternates nicely whatever order the apps are in.
const MENU_ITEMS: &[MenuItem] = &[
    MenuItem { name: "Stopwatch", state: AppState::Stopwatch },
    MenuItem { name: "Timer", state: AppState::Timer },
    MenuItem { name: "Alarm", state: AppState::Alarm },
    MenuItem { name: "Flashlight", state: AppState::Flashlight },
    MenuItem { name: "Steps", state: AppState::Steps },
    MenuItem { name: "Battery", state: AppState::Battery },
    MenuItem { name: "Calculator", state: AppState::Calculator },
    MenuItem { name: "Level", state: AppState::Level },
    MenuItem { name: "Dice", state: AppState::Dice },
    MenuItem { name: "Sun & Moon", state: AppState::SunMoon },
    MenuItem { name: "Wi-Fi QR", state: AppState::WifiQr },
    MenuItem { name: "Smart Home", state: AppState::SmartHome },
    MenuItem { name: "Home Status", state: AppState::HaStatus },
    MenuItem { name: "Weather", state: AppState::Weather },
    MenuItem { name: "Fleet", state: AppState::Fleet },
    MenuItem { name: "Waves", state: AppState::Waves },
    MenuItem { name: "Ball Pit", state: AppState::Fluid },
    MenuItem { name: "Asteroids", state: AppState::Asteroids },
    MenuItem { name: "Tic-Tac-Toe", state: AppState::TicTacToe },
    MenuItem { name: "Tetris", state: AppState::Tetris },
    MenuItem { name: "Maze (Tilt)", state: AppState::Maze },
    MenuItem { name: "Settings", state: AppState::Settings },
];

pub struct Launcher {
    scroll: f32,          // current scroll offset, px
    target: f32,          // where the scroll is gliding to
    dragging: bool,
    drag_start_y: i32,
    drag_start_scroll: f32,
    last_y: i32,
    velocity: f32,        // px per frame while dragging (for the fling)
    moved: bool,          // this touch turned into a drag (so it's not a tap)
}

const DRAG_SLOP: i32 = 12;     // px of movement before a touch becomes a drag
const FLING: f32 = 9.0;        // how far a flick keeps going (frames of velocity)

impl Launcher {
    pub fn new() -> Self {
        Self { scroll: 0.0, target: 0.0, dragging: false, drag_start_y: 0, drag_start_scroll: 0.0,
               last_y: 0, velocity: 0.0, moved: false }
    }

    fn max_scroll() -> f32 {
        ((MENU_ITEMS.len() as i32) * (ITEM_H + ITEM_GAP) + START_Y - 470).max(0) as f32
    }

    /// True while the list is still gliding (the main loop then ticks at ~60 Hz).
    pub fn is_animating(&self) -> bool {
        self.dragging || (self.target - self.scroll).abs() > 0.5
    }

    /// `finger_y`: Some(y) while a finger is on the screen (from the main touch poll).
    /// Phone-style: the list follows your finger, flicks keep gliding, taps open apps.
    pub fn update(&mut self, finger_y: Option<u16>, swipe: Option<SwipeDirection>, tap: bool, tap_y: u16) -> Option<AppState> {
        let max = Self::max_scroll();
        match finger_y {
            Some(y) => {
                let y = y as i32;
                if !self.dragging {
                    self.dragging = true;
                    self.moved = false;
                    self.drag_start_y = y;
                    self.drag_start_scroll = self.scroll;
                    self.last_y = y;
                    self.velocity = 0.0;
                    self.target = self.scroll; // catch a gliding list
                } else {
                    let dy = (self.last_y - y) as f32;
                    self.velocity = self.velocity * 0.5 + dy * 0.5;
                    self.last_y = y;
                    if (y - self.drag_start_y).abs() > DRAG_SLOP { self.moved = true; }
                    if self.moved {
                        // Follow the finger, with a little rubber-band past the ends.
                        let raw = self.drag_start_scroll + (self.drag_start_y - y) as f32;
                        self.scroll = if raw < 0.0 { raw / 3.0 } else if raw > max { max + (raw - max) / 3.0 } else { raw };
                        self.target = self.scroll;
                    }
                }
            }
            None => {
                if self.dragging {
                    self.dragging = false;
                    if self.moved {
                        self.target = self.scroll + self.velocity * FLING;
                    }
                }
            }
        }

        if let Some(SwipeDirection::Right) = swipe {
            if !self.moved { return Some(AppState::Watchface); }
        }

        // Glide toward the target (fast ease-out), clamped to the list.
        self.target = self.target.clamp(0.0, max);
        if !self.dragging {
            let diff = self.target - self.scroll;
            if diff.abs() > 0.5 { self.scroll += diff * 0.35; } else { self.scroll = self.target; }
        }

        // A tap (not a drag) opens the app under the finger.
        if tap && !self.moved && !self.is_animating() {
            let y = tap_y as i32 + self.scroll as i32;
            for (i, item) in MENU_ITEMS.iter().enumerate() {
                let item_y = START_Y + i as i32 * (ITEM_H + ITEM_GAP);
                if y >= item_y && y < item_y + ITEM_H {
                    return Some(item.state);
                }
            }
        }
        if finger_y.is_none() && !self.dragging { self.moved = false; }
        None
    }

    pub fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        let _ = Rectangle::new(Point::zero(), Size::new(410, 502))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::new(1, 2, 2)))
            .draw(d);

        // Title
        let title = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
        let _ = Text::with_alignment("APPS", Point::new(205, 35), title, Alignment::Center).draw(d);

        // Menu items
        for (i, item) in MENU_ITEMS.iter().enumerate() {
            let y = START_Y + i as i32 * (ITEM_H + ITEM_GAP) - self.scroll as i32;
            if y + ITEM_H < 0 || y > 502 { continue; }
            let (fill, accent) = crate::apps::widgets::palette(i);

            // Dark background with colored accent
            let _ = RoundedRectangle::with_equal_corners(
                Rectangle::new(Point::new(MARGIN_X, y), Size::new((SCREEN_W - 2 * MARGIN_X) as u32, ITEM_H as u32)),
                Size::new(12, 12),
            ).into_styled(PrimitiveStyle::with_fill(fill)).draw(d);

            // Lighter accent bar on the left
            let _ = Rectangle::new(Point::new(MARGIN_X + 8, y + 12), Size::new(5, (ITEM_H - 24) as u32))
                .into_styled(PrimitiveStyle::with_fill(accent)).draw(d);

            // White label reads well on every palette fill
            let text_style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
            let _ = Text::with_alignment(
                item.name,
                Point::new(SCREEN_W / 2, y + ITEM_H / 2 + 5),
                text_style,
                Alignment::Center,
            ).draw(d);
        }
    }
}
