// Asteroids: your ship sits in the middle. Tilt the watch left/right to turn,
// tap to fire. Big rocks split into smaller ones. 3 lives; waves get bigger.

use core::fmt::Write;

use embassy_time::Instant;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle};

use crate::apps::widgets::{self, SBuf, CX, H, W};
use crate::apps::{App, AppInput, AppResult};

const CY: i32 = 260;
const MAX_ROCKS: usize = 16;
const MAX_SHOTS: usize = 6;
const SHOT_SPEED: f32 = 9.0;
const SHOT_LIFE: u8 = 34;
const SHIP_R: f32 = 12.0;
// Play area (inside the rounded screen, below the score line)
const TOP: f32 = 56.0;
const BOT: f32 = (H - 12) as f32;
const LEFT: f32 = 12.0;
const RIGHT: f32 = (W - 12) as f32;

#[derive(Clone, Copy)]
struct Rock { x: f32, y: f32, vx: f32, vy: f32, size: u8, shape: [u8; 8], alive: bool }
#[derive(Clone, Copy)]
struct Shot { x: f32, y: f32, vx: f32, vy: f32, life: u8 }

pub struct AsteroidsApp {
    angle: f32,      // radians, 0 = up
    rocks: [Rock; MAX_ROCKS],
    shots: [Shot; MAX_SHOTS],
    score: u32,
    lives: u8,
    wave: u8,
    invuln: u16,
    game_over: bool,
    rng: u32,
}

fn radius(size: u8) -> f32 { match size { 3 => 32.0, 2 => 19.0, _ => 10.0 } }

impl AsteroidsApp {
    pub fn new() -> Self {
        let mut a = Self {
            angle: 0.0,
            rocks: [Rock { x: 0.0, y: 0.0, vx: 0.0, vy: 0.0, size: 0, shape: [0; 8], alive: false }; MAX_ROCKS],
            shots: [Shot { x: 0.0, y: 0.0, vx: 0.0, vy: 0.0, life: 0 }; MAX_SHOTS],
            score: 0, lives: 3, wave: 0, invuln: 0, game_over: false, rng: 0xC0FF_EE11,
        };
        a.next_wave();
        a
    }

    fn rand(&mut self) -> u32 {
        let mut x = self.rng; x ^= x << 13; x ^= x >> 17; x ^= x << 5; self.rng = x; x
    }
    fn randf(&mut self) -> f32 { (self.rand() % 10_000) as f32 / 10_000.0 }

    fn spawn(&mut self, x: f32, y: f32, size: u8, speed: f32) {
        let ang = self.randf() * 6.2832;
        let mut shape = [0u8; 8];
        for s in shape.iter_mut() { *s = 70 + (self.rand() % 40) as u8; } // 70..110 % of radius
        if let Some(r) = self.rocks.iter_mut().find(|r| !r.alive) {
            *r = Rock { x, y, vx: libm::sinf(ang) * speed, vy: libm::cosf(ang) * speed, size, shape, alive: true };
        }
    }

    fn next_wave(&mut self) {
        self.wave += 1;
        let n = (self.wave + 2).min(7);
        for _ in 0..n {
            // Spawn along the edges, away from the ship.
            let edge = self.rand() % 4;
            let t = self.randf();
            let (x, y) = match edge {
                0 => (LEFT + t * (RIGHT - LEFT), TOP),
                1 => (LEFT + t * (RIGHT - LEFT), BOT),
                2 => (LEFT, TOP + t * (BOT - TOP)),
                _ => (RIGHT, TOP + t * (BOT - TOP)),
            };
            let sp = 0.9 + self.randf() * 0.6 + self.wave as f32 * 0.1;
            self.spawn(x, y, 3, sp);
        }
        self.invuln = 60;
    }

    fn restart(&mut self) {
        let seed = self.rng ^ Instant::now().as_ticks() as u32;
        *self = Self::new();
        self.rng = seed | 1;
    }

    fn wrap(v: &mut f32, lo: f32, hi: f32) {
        if *v < lo { *v += hi - lo; } else if *v > hi { *v -= hi - lo; }
    }

    pub fn is_over(&self) -> bool { self.game_over }
}

impl App for AsteroidsApp {
    fn name(&self) -> &str { "Asteroids" }
    fn setup(&mut self) { self.restart(); }

    fn update(&mut self, input: &AppInput) -> AppResult {
        if self.game_over {
            if input.tap { self.restart(); }
            return AppResult::Continue;
        }
        // Tilt to turn (same axis as the watchface gyro ball), small dead zone.
        let tilt = input.accel.1;
        if tilt.abs() > 0.08 { self.angle -= tilt * 0.16; }

        if input.tap {
            let (s, c) = (libm::sinf(self.angle), libm::cosf(self.angle));
            if let Some(sh) = self.shots.iter_mut().find(|s| s.life == 0) {
                *sh = Shot { x: CX as f32 + s * 16.0, y: CY as f32 - c * 16.0, vx: s * SHOT_SPEED, vy: -c * SHOT_SPEED, life: SHOT_LIFE };
            }
        }
        for sh in self.shots.iter_mut().filter(|s| s.life > 0) {
            sh.x += sh.vx; sh.y += sh.vy; sh.life -= 1;
            Self::wrap(&mut sh.x, LEFT, RIGHT); Self::wrap(&mut sh.y, TOP, BOT);
        }
        for r in self.rocks.iter_mut().filter(|r| r.alive) {
            r.x += r.vx; r.y += r.vy;
            Self::wrap(&mut r.x, LEFT, RIGHT); Self::wrap(&mut r.y, TOP, BOT);
        }

        // Shots vs rocks
        let mut splits: [(f32, f32, u8); 8] = [(0.0, 0.0, 0); 8];
        let mut ns = 0;
        for si in 0..MAX_SHOTS {
            if self.shots[si].life == 0 { continue; }
            for ri in 0..MAX_ROCKS {
                let r = self.rocks[ri];
                if !r.alive { continue; }
                let (dx, dy) = (self.shots[si].x - r.x, self.shots[si].y - r.y);
                let rr = radius(r.size);
                if dx * dx + dy * dy < rr * rr {
                    self.rocks[ri].alive = false;
                    self.shots[si].life = 0;
                    self.score += match r.size { 3 => 20, 2 => 50, _ => 100 };
                    if r.size > 1 && ns < splits.len() { splits[ns] = (r.x, r.y, r.size - 1); ns += 1; }
                    break;
                }
            }
        }
        for &(x, y, s) in &splits[..ns] {
            let sp = 1.4 + self.randf() * 0.8;
            self.spawn(x, y, s, sp);
            self.spawn(x, y, s, sp);
        }

        // Rocks vs ship
        if self.invuln > 0 {
            self.invuln -= 1;
        } else {
            let hit = self.rocks.iter().any(|r| {
                let (dx, dy) = (r.x - CX as f32, r.y - CY as f32);
                let d = radius(r.size) + SHIP_R - 4.0;
                r.alive && dx * dx + dy * dy < d * d
            });
            if hit {
                self.lives = self.lives.saturating_sub(1);
                if self.lives == 0 { self.game_over = true; }
                // Push nearby rocks away so you're not hit again instantly.
                for r in self.rocks.iter_mut().filter(|r| r.alive) {
                    let (dx, dy) = (r.x - CX as f32, r.y - CY as f32);
                    if dx * dx + dy * dy < 90.0 * 90.0 { r.x = if dx < 0.0 { LEFT } else { RIGHT }; }
                }
                self.invuln = 90;
            }
        }

        if !self.rocks.iter().any(|r| r.alive) { self.next_wave(); }
        AppResult::Continue
    }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        widgets::clear(d, Rgb565::BLACK);
        let mut s: SBuf<24> = SBuf::new();
        let _ = write!(s, "{}   WAVE {}", self.score, self.wave);
        widgets::text_center(d, s.as_str(), 36, widgets::AMBER_LIGHT);
        // Lives as small bars on the score line edges
        for i in 0..self.lives as i32 {
            let _ = Rectangle::new(Point::new(70 + i * 14, 24), Size::new(8, 14))
                .into_styled(PrimitiveStyle::with_fill(widgets::TEAL_LIGHT)).draw(d);
        }

        let rock_style = PrimitiveStyle::with_stroke(Rgb565::CSS_LIGHT_GRAY, 2);
        for r in self.rocks.iter().filter(|r| r.alive) {
            let rr = radius(r.size);
            let mut pts = [Point::zero(); 8];
            for (i, p) in pts.iter_mut().enumerate() {
                let a = i as f32 * 0.7854;
                let k = rr * r.shape[i] as f32 / 100.0;
                *p = Point::new((r.x + libm::sinf(a) * k) as i32, (r.y + libm::cosf(a) * k) as i32);
            }
            for i in 0..8 {
                let _ = Line::new(pts[i], pts[(i + 1) % 8]).into_styled(rock_style).draw(d);
            }
        }
        for sh in self.shots.iter().filter(|s| s.life > 0) {
            let _ = Rectangle::new(Point::new(sh.x as i32 - 2, sh.y as i32 - 2), Size::new(4, 4))
                .into_styled(PrimitiveStyle::with_fill(widgets::AMBER_LIGHT)).draw(d);
        }

        // Ship (blinks while invulnerable)
        if !self.game_over && (self.invuln / 6) % 2 == 0 {
            let (s, c) = (libm::sinf(self.angle), libm::cosf(self.angle));
            let p = |fx: f32, fy: f32| Point::new((CX as f32 + fx * c - fy * s) as i32, (CY as f32 + fx * s + fy * c) as i32);
            let nose = p(0.0, -16.0);
            let l = p(-10.0, 11.0);
            let rgt = p(10.0, 11.0);
            let back = p(0.0, 6.0);
            let st = PrimitiveStyle::with_stroke(widgets::TEAL_LIGHT, 3);
            let _ = Line::new(nose, l).into_styled(st).draw(d);
            let _ = Line::new(nose, rgt).into_styled(st).draw(d);
            let _ = Line::new(l, back).into_styled(st).draw(d);
            let _ = Line::new(rgt, back).into_styled(st).draw(d);
        }

        if self.game_over {
            widgets::text_center(d, "GAME OVER", 230, widgets::CORAL_LIGHT);
            widgets::text_center(d, "tap to play again", 262, Rgb565::WHITE);
        } else if self.wave == 1 && self.score == 0 {
            widgets::text_center(d, "tilt = turn   tap = fire", 470, Rgb565::CSS_DIM_GRAY);
        }
    }
}
