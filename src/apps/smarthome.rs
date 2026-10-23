// Smart Home / API controller
// Configurable buttons that send HTTP requests when tapped
// Perfect for Home Assistant, domotics, custom APIs

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle, RoundedRectangle};
use embedded_graphics::mono_font::ascii::FONT_10X20;
use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::text::{Alignment, Text};
use embedded_graphics::geometry::Point as EgPoint;

use crate::apps::{App, AppInput, AppResult};
use crate::peripherals::touch::SwipeDirection;

const W: i32 = 410;
const H: i32 = 502;
const BTN_H: i32 = 55;
const BTN_GAP: i32 = 6;
const BTN_MARGIN: i32 = 15;
const MAX_BUTTONS: usize = 8;

#[derive(Clone, Copy, PartialEq)]
pub enum HttpMethod {
    Get,
    Post,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ButtonState {
    Idle,
    Sending,
    Success,
    Error,
}

pub struct ApiButton {
    pub name: [u8; 16],
    pub name_len: usize,
    pub url: [u8; 96],
    pub url_len: usize,
    pub method: HttpMethod,
    pub state: ButtonState,
    pub last_response: [u8; 32],
    pub response_len: usize,
}

impl ApiButton {
    pub const fn empty() -> Self {
        Self {
            name: [0; 16], name_len: 0,
            url: [0; 96], url_len: 0,
            method: HttpMethod::Get,
            state: ButtonState::Idle,
            last_response: [0; 32], response_len: 0,
        }
    }

    pub fn new(name: &str, url: &str, method: HttpMethod) -> Self {
        let mut btn = Self::empty();
        let nb = name.as_bytes();
        let nl = nb.len().min(16);
        btn.name[..nl].copy_from_slice(&nb[..nl]);
        btn.name_len = nl;
        let ub = url.as_bytes();
        let ul = ub.len().min(96);
        btn.url[..ul].copy_from_slice(&ub[..ul]);
        btn.url_len = ul;
        btn.method = method;
        btn
    }

    fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }

    fn response_str(&self) -> &str {
        if self.response_len > 0 {
            core::str::from_utf8(&self.last_response[..self.response_len]).unwrap_or("")
        } else {
            match self.state {
                ButtonState::Idle => "",
                ButtonState::Sending => "...",
                ButtonState::Success => "OK",
                ButtonState::Error => "ERR",
            }
        }
    }
}

pub struct SmartHomeApp {
    buttons: [ApiButton; MAX_BUTTONS],
    count: usize,
    scroll_offset: i32,
    selected: Option<usize>, // button being tapped
}

impl SmartHomeApp {
    pub fn new() -> Self {
        // Default buttons - user can customize these
        let mut app = Self {
            buttons: [ApiButton::empty(), ApiButton::empty(), ApiButton::empty(), ApiButton::empty(),
                      ApiButton::empty(), ApiButton::empty(), ApiButton::empty(), ApiButton::empty()],
            count: 0,
            scroll_offset: 0,
            selected: None,
        };

        // === Home Assistant webhooks ===
        // The Home Assistant address comes from the HA_HOST build variable
        // (see ha_info.rs). Change each webhook id to match the `webhook_id`
        // of the automation it should trigger.
        // Constraints of the tiny HTTP client in peripherals/http.rs:
        //   - plain HTTP only (no TLS), so use the LAN address, not Nabu Casa
        //   - literal IPv4 only (no DNS), e.g. 192.168.1.50:8123
        //   - no auth headers, which is exactly why webhooks are used here
        // Button label: max 16 chars. URL: max 96 chars.
        for (label, id) in [
            ("Front Door Plug", "watch_front_door_plug"),
            ("Desk Light", "watch_desk_light"),
            ("Bedroom Lamp", "watch_bedroom_lamp"),
            ("Light Bar", "watch_light_bar"),
        ] {
            let url = alloc::format!("http://{}/api/webhook/{}", crate::apps::ha_info::HA_HOST, id);
            app.add_button(label, &url, HttpMethod::Post);
        }

        app
    }

    pub fn add_button(&mut self, name: &str, url: &str, method: HttpMethod) {
        if self.count < MAX_BUTTONS {
            self.buttons[self.count] = ApiButton::new(name, url, method);
            self.count += 1;
        }
    }

    /// Call this with the HTTP response after sending a request
    pub fn set_response(&mut self, idx: usize, response: &str, success: bool) {
        if idx < self.count {
            self.buttons[idx].state = if success { ButtonState::Success } else { ButtonState::Error };
            let bytes = response.as_bytes();
            let len = bytes.len().min(32);
            self.buttons[idx].last_response[..len].copy_from_slice(&bytes[..len]);
            self.buttons[idx].response_len = len;
        }
    }

    /// Map a tap's Y coordinate to a button index, using the same layout as
    /// `render`. Called by the main loop, which owns the touch coordinates.
    pub fn handle_tap(&mut self, y: u16) {
        let y = y as i32;
        for i in 0..self.count {
            let top = 55 + i as i32 * (BTN_H + BTN_GAP) - self.scroll_offset;
            if y >= top && y < top + BTN_H {
                self.selected = Some(i);
                return;
            }
        }
    }

    /// Take the pending request, if any: (index, url bytes, url len, method).
    /// The URL is returned by value so the caller can hold it across an await
    /// while still mutating the app to record the response.
    pub fn take_pending(&mut self) -> Option<(usize, [u8; 96], usize, HttpMethod)> {
        let idx = self.selected.take()?;
        if idx >= self.count {
            return None;
        }
        self.buttons[idx].state = ButtonState::Sending;
        Some((idx, self.buttons[idx].url, self.buttons[idx].url_len, self.buttons[idx].method))
    }
}

impl App for SmartHomeApp {
    fn name(&self) -> &str { "Smart Home" }

    fn setup(&mut self) {
        self.scroll_offset = 0;
        self.selected = None;
        for btn in &mut self.buttons[..self.count] {
            btn.state = ButtonState::Idle;
            btn.response_len = 0;
        }
    }

    fn update(&mut self, input: &AppInput) -> AppResult {
        // Scroll
        match input.swipe {
            Some(SwipeDirection::Up) => {
                let max = ((self.count as i32) * (BTN_H + BTN_GAP) - 350).max(0);
                self.scroll_offset = (self.scroll_offset + 80).min(max);
            }
            Some(SwipeDirection::Down) => {
                self.scroll_offset = (self.scroll_offset - 80).max(0);
            }
            _ => {}
        }

        // Taps are hit-tested by the main loop via handle_tap(), which has the
        // touch coordinates. (The old code here just cycled through buttons.)

        AppResult::Continue
    }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        let _ = Rectangle::new(EgPoint::zero(), Size::new(W as u32, H as u32))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK)).draw(d);

        let title = MonoTextStyle::new(&FONT_10X20, Rgb565::CYAN);
        let white = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
        let dim = MonoTextStyle::new(&FONT_10X20, Rgb565::CSS_GRAY);

        let _ = Text::with_alignment("SMART HOME", EgPoint::new(W / 2, 35), title, Alignment::Center).draw(d);

        for i in 0..self.count {
            let y = 55 + i as i32 * (BTN_H + BTN_GAP) - self.scroll_offset;
            if y + BTN_H < 0 || y > H { continue; }

            let btn = &self.buttons[i];

            // Shared palette (teal, amber, green, coral...) by position;
            // grey while sending, dark red on error.
            let (fill, _accent) = crate::apps::widgets::palette(i);
            let bg = match btn.state {
                ButtonState::Idle | ButtonState::Success => fill,
                ButtonState::Sending => crate::apps::widgets::SLATE,
                ButtonState::Error => Rgb565::new(15, 3, 3),
            };

            let _ = RoundedRectangle::with_equal_corners(
                Rectangle::new(EgPoint::new(BTN_MARGIN, y), Size::new((W - 2 * BTN_MARGIN) as u32, BTN_H as u32)),
                Size::new(10, 10),
            ).into_styled(PrimitiveStyle::with_fill(bg)).draw(d);

            // Method indicator
            let method_str = match btn.method {
                HttpMethod::Get => "GET",
                HttpMethod::Post => "POST",
            };
            let _ = Text::new(method_str, EgPoint::new(BTN_MARGIN + 10, y + 22),
                MonoTextStyle::new(&FONT_10X20, Rgb565::new(24, 50, 24))).draw(d);

            // Button name
            let _ = Text::with_alignment(btn.name_str(), EgPoint::new(W / 2, y + 22), white, Alignment::Center).draw(d);

            // Response/status
            let resp = btn.response_str();
            if !resp.is_empty() {
                // White reads on every palette fill; yellow while waiting.
                let resp_color = match btn.state {
                    ButtonState::Sending => Rgb565::YELLOW,
                    _ => Rgb565::WHITE,
                };
                let _ = Text::with_alignment(resp, EgPoint::new(W - BTN_MARGIN - 10, y + 22),
                    MonoTextStyle::new(&FONT_10X20, resp_color), Alignment::Right).draw(d);
            }
        }

        // Footer
        let _ = Text::with_alignment("TAP to send request", EgPoint::new(W / 2, H - 20), dim, Alignment::Center).draw(d);
    }
}
