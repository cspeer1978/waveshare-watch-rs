// Ball Pit (was "Fluid"): a few hundred balls that slosh around the screen as you
// tilt and shake the watch. Tap to change the liquid (water / lava / slime).
//
// Method: "double density relaxation" particle fluid (Clavet et al. 2005), the
// classic cheap-but-convincing liquid used in phone/watch toys:
//   1. gravity (from the accelerometer) -> velocity
//   2. predict positions
//   3. neighbours found with a uniform grid (counting sort, no allocation)
//   4. push close particles apart / pull them together toward a rest density
//   5. keep them inside the rounded screen, derive velocity from movement
// All f32 (the ESP32-S3 has a single-precision FPU).

use alloc::vec;
use alloc::vec::Vec;

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, PrimitiveStyle};

use crate::apps::widgets::{self, H, W};
use crate::apps::{App, AppInput, AppResult};

pub const N: usize = 280;           // particle count
const HR: f32 = 16.0;               // interaction radius (px)
const REST: f32 = 1.0;              // rest density
const K: f32 = 0.4;                // pressure stiffness
const K_NEAR: f32 = 3.0;            // near-pressure (stops clumping)
const GRAVITY: f32 = 0.5;          // px/frame² at 1 g
const DAMP: f32 = 0.995;            // tiny velocity damping
const DOT_R: u32 = 7;               // drawn particle radius
const MARGIN: f32 = 7.0;            // keep dots off the bezel
const CORNER_R: f32 = 62.0;         // rounded screen corner radius
const CELL: f32 = HR;
const GW: usize = (W as usize + HR as usize - 1) / HR as usize + 1;
const GH: usize = (H as usize + HR as usize - 1) / HR as usize + 1;

pub struct FluidApp {
    // Heap-allocated (PSRAM) so the ~14 KB of particle state doesn't sit in
    // the main task's internal RAM or get built on the stack.
    x: Vec<f32>,
    y: Vec<f32>,
    px: Vec<f32>,
    py: Vec<f32>,
    vx: Vec<f32>,
    vy: Vec<f32>,
    cell_start: Vec<u16>,
    sorted: Vec<u16>,
    theme: u8,
    last_accel: (f32, f32, f32),
    moving: bool,
}

impl FluidApp {
    pub fn new() -> Self {
        let mut f = Self {
            x: vec![0.0; N], y: vec![0.0; N], px: vec![0.0; N], py: vec![0.0; N], vx: vec![0.0; N], vy: vec![0.0; N],
            cell_start: vec![0; GW * GH + 1], sorted: vec![0; N], theme: 0, last_accel: (0.0, 0.0, 1.0), moving: false,
        };
        f.reset();
        f
    }

    /// Start as a block of liquid in the upper middle, so it falls and splashes.
    fn reset(&mut self) {
        let cols = 24;
        for i in 0..N {
            let c = (i % cols) as f32;
            let r = (i / cols) as f32;
            self.x[i] = 205.0 - cols as f32 * 5.0 + c * 10.0 + if (i / cols) % 2 == 0 { 0.0 } else { 5.0 };
            self.y[i] = 70.0 + r * 9.0;
            self.vx[i] = 0.0;
            self.vy[i] = 0.0;
        }
    }

    /// True if the watch moved noticeably since last frame (keeps the screen awake).
    pub fn is_moving(&self) -> bool { self.moving }

    /// Particle positions (for tests / debugging).
    pub fn positions(&self) -> (&[f32], &[f32]) { (&self.x, &self.y) }

    pub fn next_theme(&mut self) { self.theme = (self.theme + 1) % 3; }

    fn build_grid(&mut self) {
        let mut counts = [0u16; GW * GH + 1];
        let cell_of = |x: f32, y: f32| -> usize {
            let cx = ((x / CELL) as usize).min(GW - 1);
            let cy = ((y / CELL) as usize).min(GH - 1);
            cy * GW + cx
        };
        for i in 0..N { counts[cell_of(self.px[i], self.py[i])] += 1; }
        let mut sum = 0u16;
        for c in 0..GW * GH { self.cell_start[c] = sum; sum += counts[c]; }
        self.cell_start[GW * GH] = sum;
        let mut fill = [0u16; GW * GH];
        for i in 0..N {
            let c = cell_of(self.px[i], self.py[i]);
            self.sorted[(self.cell_start[c] + fill[c]) as usize] = i as u16;
            fill[c] += 1;
        }
    }

    /// Keep a point inside the rounded-rectangle screen.
    fn confine(x: &mut f32, y: &mut f32) {
        let (l, r, t, b) = (MARGIN, W as f32 - MARGIN, MARGIN, H as f32 - MARGIN);
        *x = x.clamp(l, r);
        *y = y.clamp(t, b);
        let cr = CORNER_R - MARGIN;
        let cx = if *x < l + cr { l + cr } else if *x > r - cr { r - cr } else { return };
        let cy = if *y < t + cr { t + cr } else if *y > b - cr { b - cr } else { return };
        let (dx, dy) = (*x - cx, *y - cy);
        let d2 = dx * dx + dy * dy;
        if d2 > cr * cr {
            let d = libm::sqrtf(d2);
            *x = cx + dx / d * cr;
            *y = cy + dy / d * cr;
        }
    }

    /// One simulation step. `accel` in g (watch axes).
    pub fn step(&mut self, accel: (f32, f32, f32)) {
        // Same axis mapping as the watchface gyro ball: screen x <- -accel_y, y <- accel_x.
        let gx = -accel.1 * GRAVITY;
        let gy = accel.0 * GRAVITY;
        let da = (accel.0 - self.last_accel.0).abs() + (accel.1 - self.last_accel.1).abs() + (accel.2 - self.last_accel.2).abs();
        self.moving = da > 0.04;
        // Shakes need no special code: the accelerometer reading already includes
        // the watch's own motion, so a jerk briefly changes the effective gravity.
        self.last_accel = accel;

        for i in 0..N {
            self.vx[i] = (self.vx[i] + gx) * DAMP;
            self.vy[i] = (self.vy[i] + gy) * DAMP;
            // Speed limit keeps the solver stable on hard shakes.
            let v2 = self.vx[i] * self.vx[i] + self.vy[i] * self.vy[i];
            if v2 > 256.0 { let s = 16.0 / libm::sqrtf(v2); self.vx[i] *= s; self.vy[i] *= s; }
            self.px[i] = self.x[i] + self.vx[i];
            self.py[i] = self.y[i] + self.vy[i];
            Self::confine(&mut self.px[i], &mut self.py[i]);
        }

        self.build_grid();

        // Double density relaxation.
        for i in 0..N {
            let (xi, yi) = (self.px[i], self.py[i]);
            let cx = ((xi / CELL) as isize).min(GW as isize - 1);
            let cy = ((yi / CELL) as isize).min(GH as isize - 1);
            let mut rho = 0.0f32;
            let mut rho_near = 0.0f32;
            // pass 1: densities
            for oy in -1..=1 {
                let yy = cy + oy;
                if yy < 0 || yy >= GH as isize { continue; }
                for ox in -1..=1 {
                    let xx = cx + ox;
                    if xx < 0 || xx >= GW as isize { continue; }
                    let c = yy as usize * GW + xx as usize;
                    for k in self.cell_start[c]..self.cell_start[c + 1] {
                        let j = self.sorted[k as usize] as usize;
                        if j == i { continue; }
                        let (dx, dy) = (self.px[j] - xi, self.py[j] - yi);
                        let d2 = dx * dx + dy * dy;
                        if d2 < HR * HR {
                            let q = 1.0 - libm::sqrtf(d2) / HR;
                            rho += q * q;
                            rho_near += q * q * q;
                        }
                    }
                }
            }
            // Walls act like a layer of neighbours: they add density (so pressure
            // pushes particles off them) instead of being a hard clamp only —
            // otherwise particles squeezed against a wall climb up it in a column.
            let walls = [
                (xi - MARGIN, 1.0f32, 0.0f32),                 // left  (distance, push dir)
                (W as f32 - MARGIN - xi, -1.0, 0.0),           // right
                (yi - MARGIN, 0.0, 1.0),                       // top
                (H as f32 - MARGIN - yi, 0.0, -1.0),           // bottom
            ];
            for &(dist, _, _) in &walls {
                if dist < HR * 0.5 {
                    let q = 1.0 - dist / (HR * 0.5);
                    rho += q * q;
                    rho_near += q * q * q;
                }
            }
            let p = K * (rho - REST);
            let p_near = K_NEAR * rho_near;
            // pass 2: displacements
            let (mut dxi, mut dyi) = (0.0f32, 0.0f32);
            for oy in -1..=1 {
                let yy = cy + oy;
                if yy < 0 || yy >= GH as isize { continue; }
                for ox in -1..=1 {
                    let xx = cx + ox;
                    if xx < 0 || xx >= GW as isize { continue; }
                    let c = yy as usize * GW + xx as usize;
                    for k in self.cell_start[c]..self.cell_start[c + 1] {
                        let j = self.sorted[k as usize] as usize;
                        if j == i { continue; }
                        let (dx, dy) = (self.px[j] - xi, self.py[j] - yi);
                        let d2 = dx * dx + dy * dy;
                        if d2 < HR * HR && d2 > 1e-6 {
                            let d = libm::sqrtf(d2);
                            let q = 1.0 - d / HR;
                            let disp = 0.5 * (p * q + p_near * q * q);
                            let (ux, uy) = (dx / d * disp, dy / d * disp);
                            self.px[j] += ux;
                            self.py[j] += uy;
                            dxi -= ux;
                            dyi -= uy;
                        }
                    }
                }
            }
            for &(dist, nx, ny) in &walls {
                if dist < HR * 0.5 {
                    let q = 1.0 - dist / (HR * 0.5);
                    let disp = (p * q + p_near * q * q).max(0.0);
                    dxi += nx * disp;
                    dyi += ny * disp;
                }
            }
            self.px[i] += dxi;
            self.py[i] += dyi;
        }

        for i in 0..N {
            Self::confine(&mut self.px[i], &mut self.py[i]);
            self.vx[i] = self.px[i] - self.x[i];
            self.vy[i] = self.py[i] - self.y[i];
            self.x[i] = self.px[i];
            self.y[i] = self.py[i];
        }
    }

    fn color(&self, speed2: f32) -> Rgb565 {
        // 0 = calm, 1 = fast (foam / highlight)
        let t = (speed2 / 25.0).min(1.0);
        let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
        match self.theme {
            0 => Rgb565::new(lerp(2, 20), lerp(18, 58), lerp(26, 31)),  // deep blue -> foam white
            1 => Rgb565::new(lerp(24, 31), lerp(10, 52), lerp(0, 8)),   // lava: red -> yellow
            _ => Rgb565::new(lerp(6, 22), lerp(36, 63), lerp(4, 16)),   // slime: green -> lime
        }
    }
}

impl App for FluidApp {
    fn name(&self) -> &str { "Ball Pit" }
    fn setup(&mut self) { self.reset(); }

    fn update(&mut self, input: &AppInput) -> AppResult {
        if input.tap { self.next_theme(); }
        self.step(input.accel);
        AppResult::Continue
    }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        widgets::clear(d, Rgb565::BLACK);
        for i in 0..N {
            let s2 = self.vx[i] * self.vx[i] + self.vy[i] * self.vy[i];
            let _ = Circle::with_center(Point::new(self.x[i] as i32, self.y[i] as i32), DOT_R * 2)
                .into_styled(PrimitiveStyle::with_fill(self.color(s2)))
                .draw(d);
        }
    }
}
