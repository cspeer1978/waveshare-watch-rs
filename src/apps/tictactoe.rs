// Tic-tac-toe: you (X) vs the watch (O), or 2 players taking turns.
// The watch plays: win if it can, block you, take the center, a corner, a side.

use embassy_time::Instant;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Circle, Line, PrimitiveStyle};

use crate::apps::widgets::{self, Btn};
use crate::apps::{App, AppInput, AppResult};

const GX: i32 = 70;   // grid left
const GY: i32 = 72;   // grid top
const CELL: i32 = 90;
const MODE_BTN: Btn = Btn::new(55, 375, 150, 55);
const NEW_BTN: Btn = Btn::new(213, 375, 142, 55);
const LINES: [[usize; 3]; 8] = [[0,1,2],[3,4,5],[6,7,8],[0,3,6],[1,4,7],[2,5,8],[0,4,8],[2,4,6]];

pub struct TicTacToeApp {
    board: [u8; 9], // 0 empty, 1 X, 2 O
    turn: u8,
    two_player: bool,
    winner: u8,     // 0 none, 1 X, 2 O, 3 draw
    win_line: Option<[usize; 3]>,
    rng: u32,
    score: [u16; 3], // X wins, O wins, draws
}

impl TicTacToeApp {
    pub fn new() -> Self {
        Self { board: [0; 9], turn: 1, two_player: false, winner: 0, win_line: None, rng: 0x9e37_79b9, score: [0; 3] }
    }

    fn new_game(&mut self) {
        self.board = [0; 9];
        self.turn = 1;
        self.winner = 0;
        self.win_line = None;
    }

    fn check(&mut self) {
        for l in LINES {
            let v = self.board[l[0]];
            if v != 0 && v == self.board[l[1]] && v == self.board[l[2]] {
                self.winner = v;
                self.win_line = Some(l);
                self.score[(v - 1) as usize] += 1;
                return;
            }
        }
        if self.board.iter().all(|&c| c != 0) {
            self.winner = 3;
            self.score[2] += 1;
        }
    }

    fn find_line_move(&self, who: u8) -> Option<usize> {
        for l in LINES {
            let vals = [self.board[l[0]], self.board[l[1]], self.board[l[2]]];
            if vals.iter().filter(|&&v| v == who).count() == 2 {
                if let Some(i) = vals.iter().position(|&v| v == 0) { return Some(l[i]); }
            }
        }
        None
    }

    fn ai_move(&mut self) {
        let mv = self.find_line_move(2)
            .or_else(|| self.find_line_move(1))
            .or_else(|| if self.board[4] == 0 { Some(4) } else { None })
            .or_else(|| {
                let mut x = self.rng ^ (Instant::now().as_ticks() as u32);
                x ^= x << 13; x ^= x >> 17; x ^= x << 5;
                self.rng = x;
                let corners = [0usize, 2, 6, 8];
                let start = (x % 4) as usize;
                (0..4).map(|i| corners[(start + i) % 4]).find(|&c| self.board[c] == 0)
            })
            .or_else(|| [1usize, 3, 5, 7].into_iter().find(|&c| self.board[c] == 0));
        if let Some(i) = mv {
            self.board[i] = 2;
            self.check();
            self.turn = 1;
        }
    }

    /// Returns true if something changed.
    pub fn handle_tap(&mut self, x: u16, y: u16) -> bool {
        if MODE_BTN.hit(x, y) { self.two_player = !self.two_player; self.score = [0; 3]; self.new_game(); return true; }
        if NEW_BTN.hit(x, y) { self.new_game(); return true; }
        if self.winner != 0 { self.new_game(); return true; }
        let (x, y) = (x as i32, y as i32);
        if x < GX || y < GY || x >= GX + 3 * CELL || y >= GY + 3 * CELL { return false; }
        let i = ((y - GY) / CELL * 3 + (x - GX) / CELL) as usize;
        if self.board[i] != 0 { return false; }
        self.board[i] = self.turn;
        self.check();
        if self.winner == 0 {
            if self.two_player { self.turn = 3 - self.turn; } else { self.turn = 2; self.ai_move(); }
        }
        true
    }

    fn cell_center(i: usize) -> Point {
        Point::new(GX + (i % 3) as i32 * CELL + CELL / 2, GY + (i / 3) as i32 * CELL + CELL / 2)
    }
}

impl App for TicTacToeApp {
    fn name(&self) -> &str { "Tic-Tac-Toe" }
    fn setup(&mut self) {}
    fn update(&mut self, _input: &AppInput) -> AppResult { AppResult::Continue }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        widgets::clear(d, Rgb565::BLACK);
        let (msg, col) = match (self.winner, self.two_player) {
            (1, false) => ("You win!", widgets::GREEN_LIGHT),
            (2, false) => ("Watch wins!", widgets::CORAL_LIGHT),
            (1, true) => ("X wins!", widgets::CORAL_LIGHT),
            (2, true) => ("O wins!", widgets::TEAL_LIGHT),
            (3, _) => ("Draw", widgets::AMBER_LIGHT),
            (_, false) => ("Your turn (X)", Rgb565::WHITE),
            (_, true) => if self.turn == 1 { ("X to play", widgets::CORAL_LIGHT) } else { ("O to play", widgets::TEAL_LIGHT) },
        };
        widgets::title(d, msg, col);

        let grid = PrimitiveStyle::with_stroke(Rgb565::CSS_GRAY, 4);
        for k in 1..3 {
            let _ = Line::new(Point::new(GX + k * CELL, GY + 6), Point::new(GX + k * CELL, GY + 3 * CELL - 6)).into_styled(grid).draw(d);
            let _ = Line::new(Point::new(GX + 6, GY + k * CELL), Point::new(GX + 3 * CELL - 6, GY + k * CELL)).into_styled(grid).draw(d);
        }
        let xs = PrimitiveStyle::with_stroke(widgets::CORAL_LIGHT, 10);
        let os = PrimitiveStyle::with_stroke(widgets::TEAL_LIGHT, 10);
        for i in 0..9 {
            let c = Self::cell_center(i);
            let r = CELL / 2 - 18;
            match self.board[i] {
                1 => {
                    let _ = Line::new(c + Point::new(-r, -r), c + Point::new(r, r)).into_styled(xs).draw(d);
                    let _ = Line::new(c + Point::new(-r, r), c + Point::new(r, -r)).into_styled(xs).draw(d);
                }
                2 => { let _ = Circle::with_center(c, (r * 2) as u32).into_styled(os).draw(d); }
                _ => {}
            }
        }
        if let Some(l) = self.win_line {
            let _ = Line::new(Self::cell_center(l[0]), Self::cell_center(l[2]))
                .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 6)).draw(d);
        }

        let mut s: crate::apps::widgets::SBuf<32> = crate::apps::widgets::SBuf::new();
        use core::fmt::Write;
        if self.two_player {
            let _ = write!(s, "X {}   O {}   Draw {}", self.score[0], self.score[1], self.score[2]);
        } else {
            let _ = write!(s, "You {}   Watch {}   Draw {}", self.score[0], self.score[1], self.score[2]);
        }
        widgets::text_center(d, s.as_str(), 360, Rgb565::CSS_GRAY);
        MODE_BTN.draw(d, if self.two_player { "2 PLAYERS" } else { "VS WATCH" }, widgets::TEAL, Rgb565::WHITE);
        NEW_BTN.draw(d, "NEW GAME", widgets::AMBER, Rgb565::WHITE);
        widgets::text_center(d, "BOOT = back", 480, Rgb565::CSS_DIM_GRAY);
    }
}
