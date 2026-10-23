// Calculator: 4x5 touch keypad, classic left-to-right evaluation
// (2 + 3 x 4 = 20, like a pocket calculator). Big 7-segment display.

use core::fmt::Write;

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;

use crate::apps::widgets::{self, Btn, SBuf, CX};
use crate::apps::{App, AppInput, AppResult};

const KEYS: [[&str; 4]; 5] = [
    ["C", "DEL", "+/-", "/"],
    ["7", "8", "9", "x"],
    ["4", "5", "6", "-"],
    ["1", "2", "3", "+"],
    ["0", ".", "=", "="],
];
const GX: i32 = 25;
const GY: i32 = 140;
const KW: i32 = 84;
const KH: i32 = 56;
const KGAP: i32 = 8;
const MAX_LEN: usize = 10;

pub struct CalculatorApp {
    entry: SBuf<16>,     // what's being typed / the shown result
    acc: f64,
    op: Option<u8>,      // b'+', b'-', b'x', b'/'
    fresh: bool,         // next digit starts a new number
    error: bool,
}

fn key_rect(row: usize, col: usize) -> Btn {
    if row == 4 && col >= 2 {
        // "=" spans two columns
        return Btn::new(GX + 2 * (KW + KGAP), GY + 4 * (KH + KGAP), 2 * KW + KGAP, KH);
    }
    Btn::new(GX + col as i32 * (KW + KGAP), GY + row as i32 * (KH + KGAP), KW, KH)
}

fn fmt_num(v: f64) -> Option<SBuf<16>> {
    if !v.is_finite() || v.abs() >= 1e10 { return None; }
    let mut b: SBuf<16> = SBuf::new();
    if v == (v as i64) as f64 {
        let _ = write!(b, "{}", v as i64);
        return Some(b);
    }
    let int_digits = { let mut n = v.abs() as i64; let mut c = 1; while n >= 10 { n /= 10; c += 1; } c };
    let decimals = (MAX_LEN as i32 - 1 - int_digits - if v < 0.0 { 1 } else { 0 }).clamp(0, 8) as usize;
    let mut tmp: SBuf<24> = SBuf::new();
    let _ = write!(tmp, "{:.*}", decimals, v);
    let s = tmp.as_str();
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.') } else { s };
    let _ = write!(b, "{}", s);
    Some(b)
}

impl CalculatorApp {
    pub fn new() -> Self {
        let mut s = Self { entry: SBuf::new(), acc: 0.0, op: None, fresh: true, error: false };
        s.set_entry("0");
        s
    }

    fn set_entry(&mut self, t: &str) {
        self.entry = SBuf::new();
        let _ = write!(self.entry, "{}", t);
    }

    fn value(&self) -> f64 {
        self.entry.as_str().parse::<f64>().unwrap_or(0.0)
    }

    fn apply(&mut self) {
        let v = self.value();
        let r = match self.op {
            Some(b'+') => self.acc + v,
            Some(b'-') => self.acc - v,
            Some(b'x') => self.acc * v,
            Some(b'/') => if v == 0.0 { f64::NAN } else { self.acc / v },
            _ => v,
        };
        match fmt_num(r) {
            Some(b) => { self.acc = r; self.entry = b; }
            None => { self.error = true; self.set_entry("0"); }
        }
    }

    fn press(&mut self, k: &str) {
        if self.error && k != "C" { return; }
        match k {
            "C" => { *self = Self::new(); }
            "DEL" => {
                if self.fresh { return; }
                let s = self.entry.as_str();
                let t: SBuf<16> = { let mut b = SBuf::new(); let cut = &s[..s.len().saturating_sub(1)];
                    let _ = write!(b, "{}", if cut.is_empty() || cut == "-" { "0" } else { cut }); b };
                self.entry = t;
            }
            "+/-" => {
                let s = self.entry.as_str();
                if s == "0" { return; }
                let t: SBuf<16> = { let mut b = SBuf::new();
                    if let Some(r) = s.strip_prefix('-') { let _ = write!(b, "{}", r); } else { let _ = write!(b, "-{}", s); } b };
                self.entry = t;
            }
            "+" | "-" | "x" | "/" => {
                if !self.fresh || self.op.is_none() { self.apply(); }
                self.op = Some(k.as_bytes()[0]);
                self.fresh = true;
            }
            "=" => {
                self.apply();
                self.op = None;
                self.fresh = true;
            }
            "." => {
                if self.fresh { self.set_entry("0."); self.fresh = false; return; }
                if !self.entry.as_str().contains('.') && self.entry.as_str().len() < MAX_LEN {
                    let _ = write!(self.entry, ".");
                }
            }
            digit => {
                if self.fresh || self.entry.as_str() == "0" {
                    self.set_entry(digit);
                    self.fresh = false;
                } else if self.entry.as_str().len() < MAX_LEN {
                    let _ = write!(self.entry, "{}", digit);
                }
            }
        }
    }

    /// Returns true if a key was hit (caller redraws).
    pub fn handle_tap(&mut self, x: u16, y: u16) -> bool {
        for (r, row) in KEYS.iter().enumerate() {
            for (c, k) in row.iter().enumerate() {
                if r == 4 && c == 3 { continue; } // second half of "="
                if key_rect(r, c).hit(x, y) {
                    self.press(k);
                    return true;
                }
            }
        }
        false
    }
}

impl App for CalculatorApp {
    fn name(&self) -> &str { "Calculator" }
    fn setup(&mut self) {}
    fn update(&mut self, _input: &AppInput) -> AppResult { AppResult::Continue }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        widgets::clear(d, Rgb565::BLACK);
        // Pending operation shown small above the number
        if let Some(op) = self.op {
            let mut s: SBuf<24> = SBuf::new();
            if let Some(a) = fmt_num(self.acc) { let _ = write!(s, "{} {}", a.as_str(), op as char); }
            widgets::text_center(d, s.as_str(), 40, Rgb565::CSS_GRAY);
        }
        if self.error {
            widgets::text_center(d, "Error", 95, widgets::CORAL_LIGHT);
        } else {
            let len = self.entry.as_str().len() as i32;
            let (dw, dh, t) = if len <= 7 { (34, 62, 6) } else { (26, 50, 5) };
            widgets::big_text(d, CX, 60, self.entry.as_str(), dw, dh, t, Rgb565::WHITE);
        }
        for (r, row) in KEYS.iter().enumerate() {
            for (c, k) in row.iter().enumerate() {
                if r == 4 && c == 3 { continue; }
                let bg = match *k {
                    "C" | "DEL" | "+/-" => widgets::SLATE,
                    "/" | "x" | "-" | "+" => widgets::AMBER,
                    "=" => widgets::CORAL,
                    _ => widgets::TEAL,
                };
                key_rect(r, c).draw(d, k, bg, Rgb565::WHITE);
            }
        }
    }
}
