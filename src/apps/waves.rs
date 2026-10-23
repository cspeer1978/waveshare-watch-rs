// Waves: liquid in a glass box, sloshing as you tilt, shake and twist the watch.
//
// A Rust port of the approach in esp32-fluidbox by V4C38
// (https://github.com/V4C38/esp32-fluidbox, MIT License), adapted for this
// 410x502 screen and this firmware's single-core main loop:
//   * 3D particles in a box with depth, Clavet double-density relaxation
//     (position based), plus linear/quadratic viscosity.
//   * Real-world gravity scaled to the screen's physical size, a capped
//     time step for stability, shake = high-passed accelerometer (applied
//     opposite, like a real glass), gyro rotation adds centrifugal / Euler /
//     Coriolis "swirl" forces.
//   * Rounded box walls with soft fillets, restitution, friction, jitter.
//   * Rendering: perspective projection, back-to-front by depth, speed+depth
//     colour table (deep blue -> pale blue -> white foam), small highlight.
// Tap to switch liquid colour (water / lava / slime).

use alloc::vec;
use alloc::vec::Vec;

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, PrimitiveStyle};

use crate::apps::widgets::{self, H, W};
use crate::apps::{App, AppInput, AppResult};

// ---------------- configuration (names follow fluidbox's config.h) ----------------
pub const N: usize = 560;              // particles (fluidbox: 900 on a dedicated core)
const BOX_W: f32 = W as f32;
const BOX_H: f32 = H as f32;
const BOX_D: f32 = 60.0;               // box depth (px)
const PX_PER_METER: f32 = 11600.0;     // 2.06" panel: ~35 mm across 410 px
const PX_PER_MM: f32 = PX_PER_METER / 1000.0;
const BOX_CORNER_R: f32 = 4.5 * PX_PER_MM;
const BOX_BACK_FILLET: f32 = 2.0 * PX_PER_MM;
const BOX_FRONT_FILLET: f32 = BOX_BACK_FILLET * 0.25;
const WALL_MARGIN: f32 = 2.0;
const TIME_SCALE: f32 = 0.10;
const SIM_DT_MAX: f32 = 0.0030;        // a bit above fluidbox's 0.0022: we step less often
const GRAVITY_MPS2: f32 = 9.81;
const GRAVITY_GAIN: f32 = 2.2;
const REST_SPACING: f32 = 18.0;
const H_R: f32 = 30.0;                 // smoothing radius
const K_PRESSURE: f32 = 400_000.0;
const K_NEAR_PRESSURE: f32 = 800_000.0;
const MAX_DISPLACEMENT: f32 = 4.0;
const WALL_JITTER: f32 = 0.35;
const VISC_SIGMA: f32 = 45.0;
const VISC_BETA: f32 = 0.03;
const WALL_RESTITUTION: f32 = 0.25;
const WALL_FRICTION: f32 = 0.96;
const GRAVITY_LP_HZ: f32 = 1.2;
const SHAKE_GAIN: f32 = 1.6;
const ROTATION_GAIN: f32 = 1.0;
// rendering
const PROJ_FOCAL: f32 = 220.0;
const PARTICLE_RADIUS_PX: f32 = 8.5;
const SPEED_COLOR_MAX: f32 = 5000.0;
const SPEED_COLOR_GAMMA: f32 = 0.55;
const DEPTH_DIM_MIN: f32 = 0.60;
const HIGHLIGHT_LIFT: f32 = 0.55;
const DEPTH_BUCKETS: usize = 12;
// grid
const GX: usize = (BOX_W / H_R) as usize + 1;
const GY: usize = (BOX_H / H_R) as usize + 1;
const GZ: usize = (BOX_D / H_R) as usize + 1;
const NCELLS: usize = GX * GY * GZ;

pub struct WavesApp {
    pos: Vec<[f32; 3]>,
    old: Vec<[f32; 3]>,
    vel: Vec<[f32; 3]>,
    delta: Vec<[f32; 3]>,
    rho: Vec<f32>,
    rhon: Vec<f32>,
    press: Vec<f32>,
    pressn: Vec<f32>,
    cell_of: Vec<u16>,
    cell_start: Vec<u16>,
    sorted: Vec<u16>,
    rest_density: f32,
    rng: u32,
    // IMU state
    lp: [f32; 3],
    lp_init: bool,
    omega: [f32; 3],
    omega_prev: [f32; 3],
    gyro_dps: [f32; 3],
    moving: bool,
    theme: u8,
    // Active particle count: starts at N, trimmed automatically if frames are slow.
    n: usize,
    frame_ema: f32,
    frames: u32,
}

fn calibrate_rest_density() -> f32 {
    let reach = libm::ceilf(H_R / REST_SPACING) as i32;
    let mut rho0 = 0.0;
    for i in -reach..=reach {
        for j in -reach..=reach {
            for k in -reach..=reach {
                if i == 0 && j == 0 && k == 0 { continue; }
                let d = REST_SPACING * libm::sqrtf((i * i + j * j + k * k) as f32);
                if d < H_R { let q = 1.0 - d / H_R; rho0 += q * q; }
            }
        }
    }
    rho0
}

impl WavesApp {
    pub fn new() -> Self {
        let mut w = Self {
            pos: vec![[0.0; 3]; N], old: vec![[0.0; 3]; N], vel: vec![[0.0; 3]; N], delta: vec![[0.0; 3]; N],
            rho: vec![0.0; N], rhon: vec![0.0; N], press: vec![0.0; N], pressn: vec![0.0; N],
            cell_of: vec![0; N], cell_start: vec![0; NCELLS + 1], sorted: vec![0; N],
            rest_density: calibrate_rest_density(), rng: 0x2545_F491,
            lp: [0.0; 3], lp_init: false, omega: [0.0; 3], omega_prev: [0.0; 3], gyro_dps: [0.0; 3],
            moving: false, theme: 0, n: N, frame_ema: 0.0, frames: 0,
        };
        w.reset();
        w
    }

    fn rand(&mut self) -> f32 {
        let mut x = self.rng; x ^= x << 13; x ^= x >> 17; x ^= x << 5; self.rng = x;
        (x >> 8) as f32 / 16_777_216.0
    }

    /// Block of liquid resting on the bottom of the box, filling upward.
    fn reset(&mut self) {
        let s = REST_SPACING;
        let nx = (((BOX_W - 2.0 * WALL_MARGIN - 2.0 * BOX_CORNER_R * 0.3) / s) as usize).max(1);
        let nz = (((BOX_D - 2.0 * WALL_MARGIN) / s) as usize).max(1);
        let per_layer = nx * nz;
        let x0 = 0.5 * (BOX_W - (nx - 1) as f32 * s);
        let z0 = 0.5 * (BOX_D - (nz - 1) as f32 * s);
        for i in 0..N {
            let layer = i / per_layer;
            let rem = i % per_layer;
            let (ix, iz) = (rem % nx, rem / nx);
            let j = [(self.rand() - 0.5) * 0.2 * s, (self.rand() - 0.5) * 0.2 * s, (self.rand() - 0.5) * 0.2 * s];
            let p = [x0 + ix as f32 * s + j[0], BOX_H - WALL_MARGIN - s * 0.5 - layer as f32 * s + j[1], z0 + iz as f32 * s + j[2]];
            self.pos[i] = p;
            self.old[i] = p;
            self.vel[i] = [0.0; 3];
        }
        self.lp_init = false;
    }

    pub fn is_moving(&self) -> bool { self.moving }

    /// Report how long the last frame took (sim + draw + flush). If the watch
    /// can't keep up (~22 fps), drop particles until it can — never below 260.
    pub fn report_frame_ms(&mut self, ms: u32) {
        self.frames += 1;
        if self.frames < 10 { return; } // ignore start-up frames
        let ms = ms as f32;
        self.frame_ema = if self.frame_ema == 0.0 { ms } else { self.frame_ema * 0.85 + ms * 0.15 };
        if self.frame_ema > 45.0 && self.n > 260 && self.frames % 8 == 0 {
            self.n = (self.n - 30).max(260);
        }
    }

    pub fn particle_count(&self) -> usize { self.n }

    /// Gyro rates in degrees/second (watch IMU axes).
    pub fn set_gyro(&mut self, gx: f32, gy: f32, gz: f32) { self.gyro_dps = [gx, gy, gz]; }

    /// Positions (for tests).
    pub fn positions(&self) -> &[[f32; 3]] { &self.pos }

    fn cell_index(p: &[f32; 3]) -> usize {
        let cx = ((p[0] / H_R) as usize).min(GX - 1);
        let cy = ((p[1] / H_R) as usize).min(GY - 1);
        let cz = ((p[2] / H_R) as usize).min(GZ - 1);
        (cz * GY + cy) * GX + cx
    }

    fn rebuild_grid(&mut self) {
        let mut counts = vec![0u16; NCELLS];
        for i in 0..self.n { let c = Self::cell_index(&self.pos[i]); self.cell_of[i] = c as u16; counts[c] += 1; }
        let mut sum = 0u16;
        for c in 0..NCELLS { self.cell_start[c] = sum; sum += counts[c]; counts[c] = 0; }
        self.cell_start[NCELLS] = sum;
        for i in 0..self.n {
            let c = self.cell_of[i] as usize;
            self.sorted[(self.cell_start[c] + counts[c]) as usize] = i as u16;
            counts[c] += 1;
        }
    }

    /// Calls f(i, j) for every pair (j > i) in neighbouring cells.
    #[inline(always)]
    fn for_pairs(&mut self, mut f: impl FnMut(&mut Self, usize, usize)) {
        for i in 0..self.n {
            let c = self.cell_of[i] as usize;
            let (cx, cy, cz) = (c % GX, (c / GX) % GY, c / (GX * GY));
            for dz in 0..3usize {
                let z = cz + dz; if z == 0 || z > GZ { continue; } let z = z - 1;
                for dy in 0..3usize {
                    let y = cy + dy; if y == 0 || y > GY { continue; } let y = y - 1;
                    for dx in 0..3usize {
                        let x = cx + dx; if x == 0 || x > GX { continue; } let x = x - 1;
                        let cc = (z * GY + y) * GX + x;
                        let (a, b) = (self.cell_start[cc] as usize, self.cell_start[cc + 1] as usize);
                        for k in a..b {
                            let j = self.sorted[k] as usize;
                            if j > i { f(self, i, j); }
                        }
                    }
                }
            }
        }
    }

    /// Accelerometer (g, watch axes) -> gravity+shake in px/s² (sim axes), like fluidbox imu.c.
    fn forces(&mut self, accel: (f32, f32, f32), dt_real: f32) -> ([f32; 3], [f32; 3], [f32; 3]) {
        let g0 = 9.81;
        // IMU -> sim axes: x = ay, y = -ax, z = az (fluidbox mapping; matches this board's x/y).
        let a = [accel.1 * g0, -accel.0 * g0, accel.2 * g0];
        if !self.lp_init { self.lp = a; self.lp_init = true; }
        let k = 1.0 - libm::expf(-2.0 * core::f32::consts::PI * GRAVITY_LP_HZ * dt_real);
        for i in 0..3 { self.lp[i] += k * (a[i] - self.lp[i]); }
        let mut d = [-self.lp[0], -self.lp[1], -self.lp[2]];
        let m = libm::sqrtf(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
        if m > 0.5 { for v in d.iter_mut() { *v /= m; } } else { d = [0.0, 1.0, 0.0]; }
        let g_px = GRAVITY_GAIN * GRAVITY_MPS2 * PX_PER_METER;
        let shake_px = SHAKE_GAIN * PX_PER_METER;
        let mut g = [0.0; 3];
        let mut shake_mag = 0.0;
        for i in 0..3 {
            let sh = a[i] - self.lp[i];
            shake_mag += sh.abs();
            g[i] = d[i] * g_px - sh * shake_px;
        }
        self.moving = shake_mag > 0.6; // m/s² of high-passed motion
        let deg = core::f32::consts::PI / 180.0;
        let w = [self.gyro_dps[1] * deg, -self.gyro_dps[0] * deg, self.gyro_dps[2] * deg];
        if w.iter().map(|v| v.abs()).sum::<f32>() > 0.3 { self.moving = true; }
        let inv = if dt_real > 1e-6 { 1.0 / dt_real } else { 0.0 };
        let alpha = [(w[0] - self.omega_prev[0]) * inv, (w[1] - self.omega_prev[1]) * inv, (w[2] - self.omega_prev[2]) * inv];
        self.omega_prev = w;
        self.omega = w;
        (g, w, alpha)
    }

    pub fn step(&mut self, accel: (f32, f32, f32), dt_real: f32) {
        let dt_real = dt_real.clamp(0.005, 0.1);
        let (g, mut w, mut al) = self.forces(accel, dt_real);
        let dt = (dt_real * TIME_SCALE).min(SIM_DT_MAX);
        for v in w.iter_mut() { *v *= TIME_SCALE; }
        for v in al.iter_mut() { *v *= TIME_SCALE * TIME_SCALE; }
        let (cx, cy, cz) = (BOX_W * 0.5, BOX_H * 0.5, BOX_D * 0.5);
        let rot = ROTATION_GAIN != 0.0 && (w.iter().map(|v| v * v).sum::<f32>() > 1e-8 || al.iter().map(|v| v * v).sum::<f32>() > 1e-8);
        let w2 = w[0] * w[0] + w[1] * w[1] + w[2] * w[2];

        // 1. integrate velocities, 2. predict positions
        for i in 0..self.n {
            let mut acc = g;
            if rot {
                let r = [self.pos[i][0] - cx, self.pos[i][1] - cy, self.pos[i][2] - cz];
                let v = self.vel[i];
                let wr = w[0] * r[0] + w[1] * r[1] + w[2] * r[2];
                let axr = [al[1] * r[2] - al[2] * r[1], al[2] * r[0] - al[0] * r[2], al[0] * r[1] - al[1] * r[0]];
                let wxv = [w[1] * v[2] - w[2] * v[1], w[2] * v[0] - w[0] * v[2], w[0] * v[1] - w[1] * v[0]];
                for k in 0..3 {
                    acc[k] += ROTATION_GAIN * (w2 * r[k] - w[k] * wr) - ROTATION_GAIN * axr[k] - ROTATION_GAIN * 2.0 * wxv[k];
                }
            }
            for k in 0..3 {
                self.vel[i][k] += acc[k] * dt;
                self.old[i][k] = self.pos[i][k];
                self.pos[i][k] += self.vel[i][k] * dt;
            }
            self.rho[i] = 0.0;
            self.rhon[i] = 0.0;
            self.delta[i] = [0.0; 3];
        }

        // 3. neighbour grid
        self.rebuild_grid();

        // 4. densities + viscosity (one pass over pairs)
        self.for_pairs(|s, i, j| {
            let d = [s.pos[j][0] - s.pos[i][0], s.pos[j][1] - s.pos[i][1], s.pos[j][2] - s.pos[i][2]];
            let r2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            if r2 < 1e-6 || r2 >= H_R * H_R { return; }
            let r = libm::sqrtf(r2);
            let q = 1.0 - r / H_R;
            let q2 = q * q;
            s.rho[i] += q2; s.rho[j] += q2;
            s.rhon[i] += q2 * q; s.rhon[j] += q2 * q;
            let u_hat = [d[0] / r, d[1] / r, d[2] / r];
            let u = (s.vel[i][0] - s.vel[j][0]) * u_hat[0] + (s.vel[i][1] - s.vel[j][1]) * u_hat[1] + (s.vel[i][2] - s.vel[j][2]) * u_hat[2];
            if u > 0.0 {
                let dv = (0.5 * dt * q * (VISC_SIGMA * u + VISC_BETA * u * u)).min(0.5 * u);
                let imp = dv * dt;
                for k in 0..3 { s.delta[i][k] -= imp * u_hat[k]; s.delta[j][k] += imp * u_hat[k]; }
            }
        });
        for i in 0..self.n {
            for k in 0..3 { self.pos[i][k] += self.delta[i][k]; }
            self.press[i] = K_PRESSURE * (self.rho[i] - self.rest_density);
            self.pressn[i] = K_NEAR_PRESSURE * self.rhon[i];
            self.delta[i] = [0.0; 3];
        }

        // 5. pressure relaxation
        let dt2 = dt * dt;
        self.for_pairs(|s, i, j| {
            let d = [s.pos[j][0] - s.pos[i][0], s.pos[j][1] - s.pos[i][1], s.pos[j][2] - s.pos[i][2]];
            let r2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            if r2 < 1e-6 || r2 >= H_R * H_R { return; }
            let r = libm::sqrtf(r2);
            let q = 1.0 - r / H_R;
            let disp = (0.5 * dt2 * ((s.press[i] + s.press[j]) * q + (s.pressn[i] + s.pressn[j]) * q * q))
                .clamp(-MAX_DISPLACEMENT, MAX_DISPLACEMENT);
            for k in 0..3 {
                let sv = d[k] / r * disp;
                s.pos[j][k] += sv;
                s.delta[i][k] -= sv;
            }
        });

        // 6. velocity from actual motion, 7. walls
        let inv_dt = 1.0 / dt;
        for i in 0..self.n {
            for k in 0..3 {
                self.pos[i][k] += self.delta[i][k];
                self.vel[i][k] = (self.pos[i][k] - self.old[i][k]) * inv_dt;
            }
            self.resolve_wall(i);
        }
    }

    fn resolve_wall(&mut self, i: usize) {
        let p = self.pos[i];
        let side_r = BOX_CORNER_R - WALL_MARGIN;
        let (lo_x, hi_x) = (WALL_MARGIN + side_r, BOX_W - WALL_MARGIN - side_r);
        let (lo_y, hi_y) = (WALL_MARGIN + side_r, BOX_H - WALL_MARGIN - side_r);
        let a = [p[0].clamp(lo_x, hi_x), p[1].clamp(lo_y, hi_y)];
        let u = [p[0] - a[0], p[1] - a[1]];
        let r = libm::sqrtf(u[0] * u[0] + u[1] * u[1]);
        let uh = if r > 1e-6 { [u[0] / r, u[1] / r] } else { [0.0, 0.0] };
        let (lo_z, hi_z) = (WALL_MARGIN, BOX_D - WALL_MARGIN);
        let (fillet, czz) = if p[2] < lo_z + BOX_FRONT_FILLET { (BOX_FRONT_FILLET, lo_z + BOX_FRONT_FILLET) }
            else if p[2] > hi_z - BOX_BACK_FILLET { (BOX_BACK_FILLET, hi_z - BOX_BACK_FILLET) }
            else { (0.0, p[2]) };
        let lim = side_r - fillet;
        let cr = r.min(lim);
        let dr = r - cr;
        let dz = p[2] - czz;
        let dd = libm::sqrtf(dr * dr + dz * dz);
        if dd <= fillet || dd < 1e-6 { return; }
        let (nr, nz) = (dr / dd, dz / dd);
        let inset = fillet - WALL_JITTER * self.rand();
        let rr = cr + nr * inset;
        self.pos[i] = [a[0] + uh[0] * rr, a[1] + uh[1] * rr, czz + nz * inset];
        let n = [uh[0] * nr, uh[1] * nr, nz];
        let v = self.vel[i];
        let vn = v[0] * n[0] + v[1] * n[1] + v[2] * n[2];
        let mut nv = v;
        if vn > 0.0 { for k in 0..3 { nv[k] -= (1.0 + WALL_RESTITUTION) * vn * n[k]; } }
        let keep = nv[0] * n[0] + nv[1] * n[1] + nv[2] * n[2];
        for k in 0..3 { nv[k] = nv[k] * WALL_FRICTION + keep * (1.0 - WALL_FRICTION) * n[k]; }
        self.vel[i] = nv;
    }

    fn color(&self, speed: f32, depth01: f32, highlight: bool) -> Rgb565 {
        // speed ramp: deep -> bright -> pale -> white (per theme)
        let t = libm::powf((speed / SPEED_COLOR_MAX).min(1.0), SPEED_COLOR_GAMMA);
        let stops: [[f32; 3]; 4] = match self.theme {
            0 => [[0.02, 0.18, 0.55], [0.10, 0.45, 0.95], [0.55, 0.80, 1.0], [1.0, 1.0, 1.0]],
            1 => [[0.45, 0.04, 0.0], [0.90, 0.25, 0.02], [1.0, 0.65, 0.10], [1.0, 0.95, 0.70]],
            _ => [[0.05, 0.35, 0.05], [0.20, 0.75, 0.10], [0.60, 0.95, 0.35], [0.92, 1.0, 0.85]],
        };
        let f = t * 3.0;
        let i = (f as usize).min(2);
        let u = f - i as f32;
        let mut c = [0.0; 3];
        for k in 0..3 { c[k] = stops[i][k] * (1.0 - u) + stops[i + 1][k] * u; }
        let dim = 1.0 - (1.0 - DEPTH_DIM_MIN) * depth01;
        for k in 0..3 {
            c[k] *= dim;
            if highlight { c[k] += (1.0 - c[k]) * HIGHLIGHT_LIFT; }
        }
        Rgb565::new((c[0] * 31.0) as u8, (c[1] * 63.0) as u8, (c[2] * 31.0) as u8)
    }
}

impl App for WavesApp {
    fn name(&self) -> &str { "Waves" }
    fn setup(&mut self) { self.reset(); }

    fn update(&mut self, input: &AppInput) -> AppResult {
        if input.tap { self.theme = (self.theme + 1) % 3; }
        self.step(input.accel, input.dt_ms as f32 / 1000.0);
        AppResult::Continue
    }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        widgets::clear(d, Rgb565::BLACK);
        // Back-to-front by depth buckets (far plane first).
        let mut buckets: [Vec<u16>; DEPTH_BUCKETS] = Default::default();
        for i in 0..self.n {
            let b = ((self.pos[i][2] / BOX_D) * DEPTH_BUCKETS as f32) as usize;
            buckets[DEPTH_BUCKETS - 1 - b.min(DEPTH_BUCKETS - 1)].push(i as u16);
        }
        let (cx, cy) = (BOX_W * 0.5, BOX_H * 0.5);
        for bucket in buckets.iter() {
            for &ii in bucket {
                let i = ii as usize;
                let p = self.pos[i];
                let s = PROJ_FOCAL / (PROJ_FOCAL + p[2]);
                let sx = cx + (p[0] - cx) * s;
                let sy = cy + (p[1] - cy) * s;
                let r = (PARTICLE_RADIUS_PX * s).max(2.0);
                let v = self.vel[i];
                let speed = libm::sqrtf(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
                let depth01 = (p[2] / BOX_D).clamp(0.0, 1.0);
                let _ = Circle::with_center(Point::new(sx as i32, sy as i32), (r * 2.0) as u32)
                    .into_styled(PrimitiveStyle::with_fill(self.color(speed, depth01, false))).draw(d);
                // little specular highlight, up and to the left
                let hr = (r * 0.42).max(1.0);
                let _ = Circle::with_center(Point::new((sx - r * 0.35) as i32, (sy - r * 0.35) as i32), (hr * 2.0) as u32)
                    .into_styled(PrimitiveStyle::with_fill(self.color(speed, depth01, true))).draw(d);
            }
        }
    }
}
