// Home Assistant info screens: HA Status, Weather, Fleet.
//
// One app type, three instances. Each asks HA's template API (POST /api/template)
// to render a small text report, so all the "which entities?" logic runs on the
// HA server and the watch just draws lines. Nothing polls in the background:
// the main loop fetches once when you open the screen and again when you tap.
//
// Report format: one item per line, first character = style code:
//   W white   G green   R red   Y yellow   C cyan   D grey (dim)
//   B big 7-segment number (digits and '-' only; text after a space is a small unit)
//
// ===================== CONFIG (edit these) =====================

/// Home Assistant address. Plain HTTP + literal IPv4 only (same as Smart Home).
/// Home Assistant LAN address (IPv4:port, plain HTTP), baked in at build time
/// from the HA_HOST env var, e.g. HA_HOST="192.168.1.50:8123".
pub const HA_HOST: &str = match option_env!("HA_HOST") { Some(h) => h, None => "192.168.1.50:8123" };

/// Long-lived access token, baked in at build time from the HA_TOKEN env var
/// (like WIFI_SSID / WIFI_PASS). Create one in HA: your profile -> Security ->
/// Long-lived access tokens. Without it these screens show a reminder.
pub const HA_TOKEN: Option<&str> = option_env!("HA_TOKEN");

/// Temperature sensors to show on HA Status. Leave empty to auto-pick
/// (all sensors with device_class temperature, sorted by name, first 5).
pub const TEMP_ENTITIES: &[&str] = &[];

/// Door/window sensors to show. Leave empty to auto-pick (binary sensors with
/// device_class door / garage_door / window / opening, first 6).
pub const DOOR_ENTITIES: &[&str] = &[];

/// Fleet machines. Each is matched to the first binary_sensor or device_tracker
/// whose entity_id contains the name (spaces match "_" or nothing), e.g. the
/// binary_sensor that HA's Ping (ICMP) integration creates for each host.
pub const FLEET: &[&str] = &["FLIX", "WOWE", "ACEMAGIC", "MAC MINI"];

// ================================================================

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;

use crate::apps::widgets::{self, SBuf, CX};
use crate::apps::{App, AppInput, AppResult};

#[derive(Clone, Copy, PartialEq)]
pub enum HaKind {
    Status,
    Weather,
    Fleet,
}

#[derive(Clone, PartialEq)]
enum Phase {
    NeedFetch,
    WaitingWifi,
    Loading,
    Ready,
    Error(&'static str),
}

pub struct HaInfoApp {
    kind: HaKind,
    phase: Phase,
    lines: Vec<String>,
    updated: Option<(u8, u8)>, // 24h hh, mm of last good fetch
}

fn quote_list(ids: &[&str]) -> String {
    let mut s = String::from("[");
    for (i, id) in ids.iter().enumerate() {
        if i > 0 { s.push(','); }
        let _ = write!(s, "'{}'", id);
    }
    s.push(']');
    s
}

impl HaInfoApp {
    pub fn new(kind: HaKind) -> Self {
        Self { kind, phase: Phase::NeedFetch, lines: Vec::new(), updated: None }
    }

    fn title(&self) -> (&'static str, Rgb565) {
        match self.kind {
            HaKind::Status => ("HOME STATUS", widgets::TEAL_LIGHT),
            HaKind::Weather => ("WEATHER", widgets::AMBER_LIGHT),
            HaKind::Fleet => ("FLEET", widgets::GREEN_LIGHT),
        }
    }

    /// The Jinja template sent to HA for this screen.
    pub fn template(&self) -> String {
        let mut t = String::with_capacity(1400);
        t.push_str("{%- set ns = namespace(o=[]) -%}");
        match self.kind {
            HaKind::Status => {
                if TEMP_ENTITIES.is_empty() {
                    t.push_str("{%- set temps = (states.sensor|selectattr('attributes.device_class','defined')\
                        |selectattr('attributes.device_class','eq','temperature')\
                        |rejectattr('state','in',['unknown','unavailable'])|sort(attribute='name')|list)[:5] -%}");
                } else {
                    let _ = write!(t, "{{%- set temps = expand({}) -%}}", quote_list(TEMP_ENTITIES));
                }
                if DOOR_ENTITIES.is_empty() {
                    t.push_str("{%- set doors = (states.binary_sensor|selectattr('attributes.device_class','defined')\
                        |selectattr('attributes.device_class','in',['door','garage_door','window','opening'])\
                        |sort(attribute='name')|list)[:6] -%}");
                } else {
                    let _ = write!(t, "{{%- set doors = expand({}) -%}}", quote_list(DOOR_ENTITIES));
                }
                t.push_str("\
                    {%- if temps|length > 0 -%}{%- set ns.o = ns.o + ['DTemperatures'] -%}{%- endif -%}\
                    {%- for s in temps -%}{%- set ns.o = ns.o + ['W' ~ s.name[:24] ~ ': ' ~ \
                      (s.state|float(0)|round(0)|int) ~ (s.attributes.unit_of_measurement|default('')|replace('\u{b0}',''))] -%}{%- endfor -%}\
                    {%- if doors|length > 0 -%}{%- set ns.o = ns.o + ['DDoors & windows'] -%}{%- endif -%}\
                    {%- for s in doors -%}{%- set ns.o = ns.o + [('R' if s.state == 'on' else 'G') ~ s.name[:24] ~ ': ' ~ \
                      ('OPEN' if s.state == 'on' else ('closed' if s.state == 'off' else s.state))] -%}{%- endfor -%}\
                    {%- if ns.o|length == 0 -%}{%- set ns.o = ['YNo temperature or door sensors found'] -%}{%- endif -%}");
            }
            HaKind::Weather => {
                t.push_str("\
                    {%- set w = states.weather|list|first -%}\
                    {%- if w -%}\
                      {%- set ns.o = ns.o + ['C' ~ (w.state|replace('partlycloudy','partly cloudy')|replace('clear-night','clear')|replace('-',' ')|title)] -%}\
                      {%- set ns.o = ns.o + ['B' ~ (w.attributes.temperature|float(0)|round(0)|int) ~ ' ' ~ (w.attributes.temperature_unit|default('')|replace('\u{b0}',''))] -%}\
                      {%- if w.attributes.apparent_temperature is defined -%}{%- set ns.o = ns.o + ['WFeels like ' ~ (w.attributes.apparent_temperature|float(0)|round(0)|int)] -%}{%- endif -%}\
                      {%- if w.attributes.humidity is defined -%}{%- set ns.o = ns.o + ['WHumidity ' ~ w.attributes.humidity ~ '%'] -%}{%- endif -%}\
                      {%- if w.attributes.wind_speed is defined -%}{%- set ns.o = ns.o + ['WWind ' ~ (w.attributes.wind_speed|float(0)|round(0)|int) ~ ' ' ~ w.attributes.wind_speed_unit|default('')] -%}{%- endif -%}\
                      {%- set ns.o = ns.o + ['D' ~ w.name[:30]] -%}\
                    {%- else -%}{%- set ns.o = ['YNo weather entity in HA'] -%}{%- endif -%}");
            }
            HaKind::Fleet => {
                let _ = write!(t, "{{%- set names = {} -%}}", quote_list(FLEET));
                t.push_str("\
                    {%- set pool = states.binary_sensor|list + states.device_tracker|list -%}\
                    {%- for n in names -%}\
                      {%- set k = n|lower|replace(' ','_?') -%}\
                      {%- set m = pool|selectattr('entity_id','search',k)|list -%}\
                      {%- if m|length == 0 -%}{%- set ns.o = ns.o + ['D' ~ n ~ ': no HA entity'] -%}\
                      {%- else -%}{%- set st = m[0].state -%}\
                        {%- if st in ['on','home','connected'] -%}{%- set ns.o = ns.o + ['G' ~ n ~ ': online'] -%}\
                        {%- elif st in ['off','not_home','disconnected'] -%}{%- set ns.o = ns.o + ['R' ~ n ~ ': OFFLINE'] -%}\
                        {%- else -%}{%- set ns.o = ns.o + ['Y' ~ n ~ ': ' ~ st] -%}{%- endif -%}\
                      {%- endif -%}\
                    {%- endfor -%}");
            }
        }
        t.push_str("{{ ns.o|join('\\n') }}");
        t
    }

    // ---- main-loop hooks ----

    pub fn needs_fetch(&self) -> bool {
        matches!(self.phase, Phase::NeedFetch | Phase::WaitingWifi)
    }
    pub fn set_waiting_wifi(&mut self) { self.phase = Phase::WaitingWifi; }
    pub fn is_waiting_wifi(&self) -> bool { self.phase == Phase::WaitingWifi }
    pub fn set_loading(&mut self) { self.phase = Phase::Loading; }
    pub fn set_error(&mut self, msg: &'static str) { self.phase = Phase::Error(msg); }

    pub fn set_result(&mut self, status: u16, body: &str, hh: u8, mm: u8) {
        match status {
            200 => {
                self.lines.clear();
                for l in body.lines().take(12) {
                    let l = l.trim_end();
                    if !l.is_empty() { self.lines.push(String::from(l)); }
                }
                self.updated = Some((hh, mm));
                self.phase = Phase::Ready;
            }
            401 | 403 => self.phase = Phase::Error("HA rejected token (401)"),
            400 => self.phase = Phase::Error("HA template error (400)"),
            _ => self.phase = Phase::Error("HA error"),
        }
    }

    /// Tap anywhere = refresh.
    pub fn handle_tap(&mut self) {
        if !matches!(self.phase, Phase::Loading | Phase::WaitingWifi) {
            self.phase = Phase::NeedFetch;
        }
    }
}

impl App for HaInfoApp {
    fn name(&self) -> &str { self.title().0 }

    /// Opening the screen triggers one fresh fetch.
    fn setup(&mut self) {
        self.phase = Phase::NeedFetch;
    }

    fn update(&mut self, _input: &AppInput) -> AppResult {
        AppResult::Continue
    }

    fn render<D: DrawTarget<Color = Rgb565>>(&self, d: &mut D) {
        widgets::clear(d, Rgb565::BLACK);
        let (title, tc) = self.title();
        widgets::title(d, title, tc);

        if HA_TOKEN.is_none() {
            widgets::text_center(d, "No HA token in this build", 200, Rgb565::YELLOW);
            widgets::text_center(d, "Rebuild with HA_TOKEN set", 235, Rgb565::CSS_GRAY);
            widgets::text_center(d, "BOOT = back", 480, Rgb565::CSS_DIM_GRAY);
            return;
        }

        // Cached lines stay visible while refreshing.
        let mut y = 85;
        for l in &self.lines {
            let (code, text) = l.split_at(l.char_indices().nth(1).map(|(i, _)| i).unwrap_or(l.len()));
            match code {
                "B" => {
                    let (num, unit) = match text.find(' ') {
                        Some(i) => (&text[..i], text[i + 1..].trim()),
                        None => (text, ""),
                    };
                    widgets::big_text(d, CX, y, num, 50, 90, 8, Rgb565::WHITE);
                    if !unit.is_empty() {
                        widgets::text_center(d, unit, y + 118, Rgb565::CSS_GRAY);
                        y += 30;
                    }
                    y += 110;
                }
                _ => {
                    let c = match code {
                        "G" => Rgb565::GREEN,
                        "R" => Rgb565::new(31, 20, 8),
                        "Y" => Rgb565::YELLOW,
                        "C" => Rgb565::CYAN,
                        "D" => Rgb565::CSS_GRAY,
                        _ => Rgb565::WHITE,
                    };
                    // Keep inside the rounded screen: ~34 chars max.
                    let end = text.char_indices().nth(34).map(|(i, _)| i).unwrap_or(text.len());
                    widgets::text_center(d, &text[..end], y, c);
                    y += 30;
                }
            }
            if y > 420 { break; }
        }

        let mut foot: SBuf<40> = SBuf::new();
        let fc = match &self.phase {
            Phase::WaitingWifi => { let _ = write!(foot, "Connecting Wi-Fi..."); Rgb565::YELLOW }
            Phase::Loading | Phase::NeedFetch => { let _ = write!(foot, "Loading..."); Rgb565::YELLOW }
            Phase::Error(m) => { let _ = write!(foot, "{}", m); Rgb565::new(31, 20, 8) }
            Phase::Ready => {
                if let Some((h, m)) = self.updated {
                    let h12 = crate::ui::segments::to_12h(h);
                    let _ = write!(foot, "Updated {}:{:02} {}  (tap = refresh)", h12, m, if h >= 12 { "PM" } else { "AM" });
                }
                Rgb565::CSS_GRAY
            }
        };
        widgets::text_center(d, foot.as_str(), 448, fc);
        widgets::text_center(d, "BOOT = back", 480, Rgb565::CSS_DIM_GRAY);
    }
}
