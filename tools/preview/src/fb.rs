use embedded_graphics::pixelcolor::{Rgb565, RgbColor};
use embedded_graphics::prelude::*;
use crate::apps::App;

pub struct Fb { pub px: Vec<Rgb565> }
impl Fb { fn new() -> Self { Self { px: vec![Rgb565::BLACK; 410 * 502] } } }
impl OriginDimensions for Fb { fn size(&self) -> Size { Size::new(410, 502) } }
impl DrawTarget for Fb {
    type Color = Rgb565; type Error = core::convert::Infallible;
    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, it: I) -> Result<(), Self::Error> {
        for Pixel(p, c) in it { if p.x >= 0 && p.y >= 0 && p.x < 410 && p.y < 502 { self.px[p.y as usize * 410 + p.x as usize] = c; } }
        Ok(())
    }
}
fn save(fb: &Fb, name: &str) { save_to(fb, &format!("{}/out/{name}.png", env!("CARGO_MANIFEST_DIR"))); }
fn save_to(fb: &Fb, path: &str) {
    let mut img = image::RgbImage::new(410, 502);
    for (i, c) in fb.px.iter().enumerate() {
        let (x, y) = ((i % 410) as i32, (i / 410) as i32);
        // rounded-corner mask (approx panel shape, r=60)
        let r = 60; let cx = if x < r { r } else if x > 409 - r { 409 - r } else { x }; let cy = if y < r { r } else if y > 501 - r { 501 - r } else { y };
        let inside = (x - cx).pow(2) + (y - cy).pow(2) <= r * r;
        let p = if inside { [(c.r() as u32 * 255 / 31) as u8, (c.g() as u32 * 255 / 63) as u8, (c.b() as u32 * 255 / 31) as u8] } else { [40, 40, 40] };
        img.put_pixel(x as u32, y as u32, image::Rgb(p));
    }
    img.save(path).unwrap();
}
pub fn run() {
    use crate::apps::ha_info::{HaInfoApp, HaKind};
    std::fs::create_dir_all(concat!(env!("CARGO_MANIFEST_DIR"), "/out")).unwrap();
    let mut fb = Fb::new();
    if std::env::args().any(|a| a == "battery") { battery_test(&mut fb); return; }
    if std::env::args().any(|a| a == "gallery") { gallery(); return; }
    if std::env::args().any(|a| a == "settings") {
        use crate::apps::settings::*;
        let mut st = SettingsApp::new();
        st.render(&mut fb); save(&fb, "settings_off");
        st.handle_tap(200, 240); // hotspot row
        st.set_status(WifiStatus::Connected { net: MODE_HOTSPOT, ip: Some([172,20,10,3]) });
        st.render(&mut fb); save(&fb, "settings_hotspot");
        st.set_status(WifiStatus::Failed); st.handle_tap(200, 110);
        st.render(&mut fb); save(&fb, "settings_failed");
        return;
    }
    {   // ---- waves test (fluidbox port) ----
        use crate::apps::App;
        let mut w = crate::apps::waves::WavesApp::new();
        let dt = 0.040f32; // assume ~25 fps on the watch
        let check = |w: &crate::apps::waves::WavesApp, tag: &str| {
            let p = w.positions();
            let bad = p.iter().any(|q| !q[0].is_finite() || !q[1].is_finite() || !q[2].is_finite());
            let miny = p.iter().map(|q| q[1]).fold(1e9f32, f32::min);
            let maxy = p.iter().map(|q| q[1]).fold(-1e9f32, f32::max);
            let minx = p.iter().map(|q| q[0]).fold(1e9f32, f32::min);
            let maxx = p.iter().map(|q| q[0]).fold(-1e9f32, f32::max);
            println!("WAVES {tag}: nan={bad} x {minx:.0}..{maxx:.0} y {miny:.0}..{maxy:.0}");
        };
        let flat_up = (0.0f32, 0.0f32, 1.0f32);     // watch face up on a table
        let upright = (1.0f32, 0.0f32, 0.0f32);     // watch held upright (screen-down = gravity)
        let t0 = std::time::Instant::now(); let mut n = 0;
        for _ in 0..60 { w.step(upright, dt); n += 1; } check(&w, "settled upright"); w.render(&mut fb); save(&fb, "wv_0_still");
        let tilt = (0.866f32, 0.5f32, 0.0f32);
        for (k, tag) in [(3,"wv_1"),(6,"wv_2"),(10,"wv_3"),(20,"wv_4")] { for _ in 0..k { w.step(tilt, dt); n += 1; } check(&w, tag); w.render(&mut fb); save(&fb, tag); }
        // hard sideways shake: alternating jolts
        for k in 0..8 { let a = if k % 2 == 0 { (1.0, 1.5, 0.0) } else { (1.0, -1.5, 0.0) }; w.step(a, dt); n += 1; }
        check(&w, "shake"); w.render(&mut fb); save(&fb, "wv_5_shake");
        for _ in 0..6 { w.step(upright, dt); n += 1; } check(&w, "after shake"); w.render(&mut fb); save(&fb, "wv_6_after");
        // twist: gyro spin about z (screen normal) 200 dps
        w.set_gyro(0.0, 0.0, 200.0); for _ in 0..8 { w.step(upright, dt); n += 1; } w.set_gyro(0.0,0.0,0.0);
        check(&w, "twist"); w.render(&mut fb); save(&fb, "wv_7_twist");
        for _ in 0..40 { w.step(flat_up, dt); n += 1; } check(&w, "flat"); w.render(&mut fb); save(&fb, "wv_8_flat");
        let el = t0.elapsed();
        let t1 = std::time::Instant::now(); for _ in 0..20 { w.render(&mut fb); } let rt = t1.elapsed();
        println!("WAVES timing: {:.2} ms/step, {:.2} ms/render (host)", el.as_secs_f64()*1000.0/n as f64 - rt.as_secs_f64()*1000.0/20.0*0.0, rt.as_secs_f64()*1000.0/20.0);
    }
    {   // ---- fluid sim test ----
        use crate::apps::App;
        let mut f = crate::apps::fluid::FluidApp::new();
        for _ in 0..400 { f.step((1.0,0.0,0.0)); }   // straight down
        let (xs, ys) = f.positions();
        let miny = ys.iter().cloned().fold(1e9, f32::min); let maxy = ys.iter().cloned().fold(-1e9, f32::max);
        println!("METRIC depth_down {:.0} (target ~85)", maxy - miny);
        f.render(&mut fb); save(&fb, "fl_b_settled");
        // tilt 30 deg to the left: g = (cos30 down, sin30 left) -> surface should be a line tilted 30 deg
        for _ in 0..500 { f.step((0.866,0.5,0.0)); }
        let (xs, ys) = f.positions();
        // surface = topmost particle per 20px column bin; fit slope
        let mut tops = vec![1e9f32; 25];
        for k in 0..xs.len() { let b = (xs[k] / 20.0) as usize; if b < 25 { tops[b] = tops[b].min(ys[k]); } }
        let pts: Vec<(f32,f32)> = tops.iter().enumerate().filter(|(_,t)| **t < 1e8).map(|(b,t)| (b as f32*20.0+10.0, *t)).collect();
        let n = pts.len() as f32; let mx = pts.iter().map(|p|p.0).sum::<f32>()/n; let my = pts.iter().map(|p|p.1).sum::<f32>()/n;
        let slope = pts.iter().map(|p|(p.0-mx)*(p.1-my)).sum::<f32>() / pts.iter().map(|p|(p.0-mx)*(p.0-mx)).sum::<f32>();
        println!("METRIC tilt_surface_angle {:.1} deg (target ~ -30 = level surface)  cols={}", slope.atan().to_degrees(), pts.len());
        f.render(&mut fb); save(&fb, "fl_d_tilt_settled");
        for _ in 0..4 { f.step((0.0,-2.5,0.5)); } f.render(&mut fb); save(&fb, "fl_e_shake");
        for _ in 0..12 { f.step((1.0,0.0,0.0)); } f.render(&mut fb); save(&fb, "fl_f_after_shake");
        let nan = f.positions().0.iter().any(|v| !v.is_finite());
        println!("METRIC nan={}", nan);
    }
    {   // ---- stage 5 screens ----
        use crate::apps::App;
        let inp = |tap: bool, accel: (f32,f32,f32)| crate::apps::AppInput { touch: None, swipe: None, tap, accel, dt_ms: 33 };
        let mut a = crate::apps::alarm::AlarmApp::new(); a.handle_tap(200, 350, 7, 0); a.render(&mut fb); save(&fb, "n_alarm");
        a.handle_tap(150, 250, 7, 0); let _ = a.check(8, 0); a.render(&mut fb); save(&fb, "n_alarm_ring");
        println!("alarm secs_until from 6:59:30 -> {:?}", { let mut b = crate::apps::alarm::AlarmApp::new(); b.handle_tap(200,350,0,0); b.secs_until(6,59,30) });
        let mut c = crate::apps::calculator::CalculatorApp::new();
        // 12.5 x 4 - 3 = 47 ; then / 0 -> error check separately
        let key = |k: &str| -> (u16,u16) { let rows=[["C","DEL","+/-","/"],["7","8","9","x"],["4","5","6","-"],["1","2","3","+"],["0",".","=","="]];
            for (r,row) in rows.iter().enumerate() { for (ci,kk) in row.iter().enumerate() { if *kk==k { return ((25+ci as i32*92+42) as u16, (140+r as i32*64+28) as u16); } } } (0,0) };
        for seq in [&["1","2",".","5","x","4","-","3","="][..], &["C","7","/","0","="][..], &["C","1","0","/","4","="][..], &["C","9","9","9","9","9","x","9","9","9","9","9","="][..]] {
            for k in seq { let (x,y)=key(k); c.handle_tap(x,y); }
            let mut f2 = Fb::new(); c.render(&mut f2); save(&f2, &format!("calc_{}", seq.len()));
        }
        c.handle_tap(67,168); for k in ["1","2",".","5","x","4","-","3","="] { let (x,y)=key(k); c.handle_tap(x,y); }
        c.render(&mut fb); save(&fb, "n_calc");
        let mut l = crate::apps::level::LevelApp::new(); for _ in 0..20 { l.update(&inp(false,(0.10,-0.15,0.98))); } l.render(&mut fb); save(&fb, "n_level");
        let mut dd = crate::apps::dice::DiceApp::new(); dd.setup(); for _ in 0..30 { dd.update(&inp(false,(0.0,0.0,1.0))); } dd.render(&mut fb); save(&fb, "n_dice");
        let mut sm = crate::apps::sunmoon::SunMoonApp::new();
        for (y,m,d) in [(2026,10,18),(2026,6,21),(2026,12,21),(2026,3,20)] { sm.set_date(y,m,d,12); let (r,s)=sm.rise_set(); println!("sun {y}-{m:02}-{d:02}: rise {:?} set {:?} (min of day local) phase {} {:.0}%", r.map(|v|(v/60,v%60)), s.map(|v|(v/60,v%60)), sm.phase_name(), sm.illumination()*100.0); }
        sm.set_date(2026,10,18,16); sm.render(&mut fb); save(&fb, "n_sunmoon");
        let mut t = crate::apps::tictactoe::TicTacToeApp::new(); t.handle_tap(110,110); t.handle_tap(290,290); t.render(&mut fb); save(&fb, "n_ttt");
        let mut ast = crate::apps::asteroids::AsteroidsApp::new(); for i in 0..40 { ast.update(&inp(i % 10 == 0,(0.0,0.2,0.98))); } ast.render(&mut fb); save(&fb, "n_asteroids");
        let mut q = crate::apps::wifiqr::WifiQrApp::new(); q.setup(); q.render(&mut fb); save(&fb, "n_wifiqr");
        println!("qr payload: {}", crate::apps::wifiqr::WifiQrApp::payload());
        crate::ui::bigclock::draw_big_clock(&mut fb, 16, 21, 18, 10, 26, 82, false); save(&fb, "n_bigclock");
        crate::ui::bigclock::draw_big_clock(&mut fb, 9, 5, 18, 10, 26, 12, false); save(&fb, "n_bigclock2");
        let lc = crate::ui::launcher::Launcher::new(); lc.render(&mut fb); save(&fb, "n_launcher");
    }
    // centertest: measure horizontal center of lit pixels for AOD + main time
    for (label, h) in [("aod 9:05", 9u8), ("aod 12:05", 12u8), ("aod 1:05", 1u8), ("aod 10:05", 10u8), ("aod 11:11", 11u8)] {
        let mut f = Fb::new();
        let _ = crate::ui::segments::draw_hhmm(&mut f, 205, 150, h, 5, Rgb565::WHITE, Rgb565::BLACK);
        let (mut mn, mut mx) = (999, 0);
        for (i, c) in f.px.iter().enumerate() { if *c != Rgb565::BLACK { let x = i % 410; mn = mn.min(x); mx = mx.max(x); } }
        println!("{label}: lit x {mn}..{mx}, center {} (screen center 205)", (mn + mx) / 2);
        save(&f, &label.replace(' ', "_").replace(':', ""));
    }
    for (label, h) in [("main 9:05:30", 9u8), ("main 12:05:30", 12u8), ("main 1:05:30", 1u8), ("main 11:05:30", 11u8)] {
        let mut f = Fb::new();
        let _ = crate::ui::segments::draw_time(&mut f, 205, 60, h, 5, 30, Rgb565::WHITE, Rgb565::BLACK);
        let (mut mn, mut mx) = (999, 0);
        for (i, c) in f.px.iter().enumerate() { if *c != Rgb565::BLACK { let x = i % 410; mn = mn.min(x); mx = mx.max(x); } }
        println!("{label}: lit x {mn}..{mx}, center {} (screen center 205)", (mn + mx) / 2);
    }
    { use crate::peripherals::power_stats::*; let mut st = PowerStats::new();
      st.display = Some(DisplayState::Bright); st.wifi = Some(WifiMode::Off); st.battery_pct = 63; st.battery_mv = 3750;
      st.drain_x10 = Some(42); st.sleep_pct = Some(96);
      let _ = crate::ui::power_page::draw_power_page(&mut fb, &st); save(&fb, "power"); }
    let l = crate::ui::launcher::Launcher::new(); l.render(&mut fb); save(&fb, "launcher");
    let mut l2 = crate::ui::launcher::Launcher::new();
    // Simulated drag: finger from y=400 to y=150 over 10 frames, then lift (flick).
    for k in 0..=10 { l2.update(Some((400 - k * 25) as u16), None, false, 0); }
    println!("after drag: scroll moved, animating={}", l2.is_animating());
    let mut frames = 0; l2.update(None, Some(crate::peripherals::touch::SwipeDirection::Up), false, 0);
    while l2.is_animating() && frames < 200 { l2.update(None, None, false, 0); frames += 1; }
    println!("fling settled after {} frames (~{} ms at 16ms)", frames, frames * 16);
    // Tap on the first visible row should open an app; a drag must not.
    let r = l2.update(None, Some(crate::peripherals::touch::SwipeDirection::Tap), true, 90);
    println!("tap after scroll opens: {:?}", r);
    let mut l3 = crate::ui::launcher::Launcher::new();
    for k in 0..=5 { l3.update(Some((300 - k * 20) as u16), None, false, 0); }
    let r3 = l3.update(None, Some(crate::peripherals::touch::SwipeDirection::Tap), true, 200);
    println!("tap event at end of a drag opens: {:?} (should be None)", r3);
    let r4 = crate::ui::launcher::Launcher::new().update(None, Some(crate::peripherals::touch::SwipeDirection::Tap), true, 90);
    println!("plain tap at top opens: {:?}", r4);
    l2.render(&mut fb); save(&fb, "launcher_scrolled");
    let mut sw = crate::apps::stopwatch::StopwatchApp::new();
    sw.handle_tap(100, 240); std::thread::sleep(std::time::Duration::from_millis(1300)); sw.handle_tap(300,240); sw.handle_tap(300,240);
    sw.render(&mut fb); save(&fb, "stopwatch");
    let t = crate::apps::timer::TimerApp::new(); t.render(&mut fb); save(&fb, "timer");
    let st = crate::apps::steps::StepsApp::new(); let mut st = st; st.setup(); st.render(&mut fb); save(&fb, "steps");
    let mut fl = crate::apps::flashlight::FlashlightApp::new(); fl.render(&mut fb); save(&fb, "flash"); fl.handle_tap(); fl.handle_tap(); fl.render(&mut fb); save(&fb, "flash_red");
    let sh = crate::apps::smarthome::SmartHomeApp::new(); sh.render(&mut fb); save(&fb, "smarthome");
    let mut a = HaInfoApp::new(HaKind::Status);
    a.set_result(200, "DTemperatures\nWBedroom: 71F\nWLiving Room Thermostat: 70F\nWOutdoor: 58F\nDDoors & windows\nGFront Door: closed\nRGarage Door: OPEN\nGBack Door: closed\n", 11, 42);
    a.render(&mut fb); save(&fb, "ha_status");
    let mut w = HaInfoApp::new(HaKind::Weather);
    w.set_result(200, "CPartly Cloudy\nB62 F\nWFeels like 60\nWHumidity 54%\nWWind 9 mph\nDForecast Home", 11, 42);
    w.render(&mut fb); save(&fb, "weather");
    let mut f = HaInfoApp::new(HaKind::Fleet);
    f.set_result(200, "GFLIX: online\nGWOWE: online\nRACEMAGIC: OFFLINE\nDMAC MINI: no HA entity", 13, 5);
    f.render(&mut fb); save(&fb, "fleet");
    for (k, n) in [(HaKind::Status, "status"), (HaKind::Weather, "weather"), (HaKind::Fleet, "fleet")] {
        std::fs::write(format!("{}/out/tpl_{n}.j2", env!("CARGO_MANIFEST_DIR")), HaInfoApp::new(k).template()).unwrap();
    }
}

fn battery_test(fb: &mut Fb) {
    use crate::peripherals::battlog::*;
    use crate::apps::battery::BatteryApp;
    // Real log from the watch (Oct 20 drain test), written into fake flash through the real code.
    let mut log = BattLog::new(esp_storage::FlashStorage::new_host());
    for l in std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/real.csv")).unwrap().lines() {
        let f: Vec<&str> = l.split(',').collect();
        let d: Vec<u8> = f[1].split('-').map(|x| (x.parse::<u32>().unwrap() % 100) as u8).collect();
        let t: Vec<u8> = f[2].split(':').map(|x| x.parse().unwrap()).collect();
        let mut flags = 0u8;
        if f[7] == "1" { flags |= F_USB; } if f[8] == "1" { flags |= F_CHARGING; } if f[9] == "1" { flags |= F_BOOT; }
        assert!(log.append(Entry { minute: rtc_minute(d[0], d[1], d[2], t[0], t[1]), pct: f[3].parse().unwrap(), mv: f[4].parse().unwrap(),
            screen_s: f[5].parse().unwrap(), radio_s: f[6].parse().unwrap(), flags, ..Default::default() }));
    }
    // plus a new-format record with diagnostics
    assert!(log.append(Entry { minute: rtc_minute(26,10,20,23,10), pct: 40, mv: 4100, flags: F_USB|F_CHARGING, has_diag: true, sleep_s: 0, awake_s: 590, timer_wakes: 0, gpio_wakes: 0, ..Default::default() }));
    let all = log.read_all();
    println!("read {} entries, last has_diag={} awake={}", all.len(), all.last().unwrap().has_diag, all.last().unwrap().awake_s);
    for v in [4200u16, 4058, 3900, 3750, 3600, 3480, 3300, 3100, 3000] { print!("{}mV={}% ", v, volt_pct(v)); } println!();
    println!("{}", crate::apps::battery::CSV_HEADER);
    for e in &all[64..] { println!("{}", crate::apps::battery::csv_line(e).as_str()); }

    let mut app = BatteryApp::new();
    let last = *all.last().unwrap();
    app.load(all.clone(), true); app.set_live(last.minute + 2, 41, 4110, true, true);
    for s in app.sessions() { println!("session {}..{} died={} ongoing={}", s.start, s.end, s.died, s.ongoing); }
    crate::apps::App::render(&app, fb); save(fb, "real_on_usb");
    // Mid-test view: entries up to 14:41, on battery, with fake diag data on them
    let mut mid: Vec<Entry> = all.iter().cloned().filter(|e| e.seq <= 33).collect();
    for e in mid.iter_mut().skip(5) { e.has_diag = true; e.sleep_s = 560; e.awake_s = 7; e.timer_wakes = 2; }
    let m = mid.last().unwrap().clone();
    app.load(mid, true); app.set_live(m.minute + 3, 38, m.mv, false, false);
    crate::apps::App::render(&app, fb); save(fb, "real_mid_test");
}

/// Every screen of the firmware, with demo data, for the README.
fn gallery() {
    use crate::apps::App;
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/screenshots");
    std::fs::create_dir_all(dir).unwrap();
    let mut n = 0;
    let mut shot = |fb: &Fb, name: &str| { n += 1; save_to(fb, &format!("{dir}/{n:02}_{name}.png")); };
    let inp = |tap: bool, accel: (f32,f32,f32)| crate::apps::AppInput { touch: None, swipe: None, tap, accel, dt_ms: 33 };
    let mut fb = Fb::new();

    // Watch face, always-on display, big clock
    let mut wf = crate::ui::watchface::WatchFace::new();
    wf.update_time(10, 9, 30); wf.update_date(23, 10, 26); wf.update_battery(78, 3990, false);
    wf.wifi_connected = true;
    let _ = wf.render(&mut fb); shot(&fb, "watchface");
    let mut fb = Fb::new(); let _ = wf.render_aod(&mut fb); shot(&fb, "always_on");
    let mut fb = Fb::new(); crate::ui::bigclock::draw_big_clock(&mut fb, 10, 9, 23, 10, 26, 78, false); shot(&fb, "big_clock");
    // Swipe pages
    let mut fb = Fb::new(); let _ = crate::ui::pages::draw_sensors_page(&mut fb, 12, -40, 1010, 3, -1, 0, 31); shot(&fb, "sensors_page");
    let mut fb = Fb::new(); let _ = crate::ui::pages::draw_system_page(&mut fb, 3990, 78, false); shot(&fb, "system_page");
    { use crate::peripherals::power_stats::*; let mut st = PowerStats::new();
      st.display = Some(DisplayState::Bright); st.wifi = Some(WifiMode::Off); st.battery_pct = 78; st.battery_mv = 3990;
      st.drain_x10 = Some(65); st.sleep_pct = Some(96);
      let mut fb = Fb::new(); let _ = crate::ui::power_page::draw_power_page(&mut fb, &st); shot(&fb, "power_page"); }
    // Launcher
    let mut fb = Fb::new(); crate::ui::launcher::Launcher::new().render(&mut fb); shot(&fb, "launcher");
    let mut l2 = crate::ui::launcher::Launcher::new();
    for k in 0..=10 { l2.update(Some((400 - k * 25) as u16), None, false, 0); }
    l2.update(None, Some(crate::peripherals::touch::SwipeDirection::Up), false, 0);
    let mut g = 0; while l2.is_animating() && g < 200 { l2.update(None, None, false, 0); g += 1; }
    let mut fb = Fb::new(); l2.render(&mut fb); shot(&fb, "launcher_scrolled");
    // Apps, in launcher order
    let mut sw = crate::apps::stopwatch::StopwatchApp::new();
    sw.handle_tap(100, 240); std::thread::sleep(std::time::Duration::from_millis(1700)); sw.handle_tap(300,240); sw.handle_tap(300,240);
    let mut fb = Fb::new(); sw.render(&mut fb); shot(&fb, "stopwatch");
    let mut fb = Fb::new(); crate::apps::timer::TimerApp::new().render(&mut fb); shot(&fb, "timer");
    let mut a = crate::apps::alarm::AlarmApp::new(); a.handle_tap(200, 350, 7, 0);
    let mut fb = Fb::new(); a.render(&mut fb); shot(&fb, "alarm");
    a.handle_tap(150, 250, 7, 0); let _ = a.check(8, 0);
    let mut fb = Fb::new(); a.render(&mut fb); shot(&fb, "alarm_ringing");
    let mut fl = crate::apps::flashlight::FlashlightApp::new();
    let mut fb = Fb::new(); fl.render(&mut fb); shot(&fb, "flashlight");
    fl.handle_tap(); fl.handle_tap(); let mut fb = Fb::new(); fl.render(&mut fb); shot(&fb, "flashlight_red");
    let mut st = crate::apps::steps::StepsApp::new(); st.setup(); let mut fb = Fb::new(); st.render(&mut fb); shot(&fb, "steps");
    let mut c = crate::apps::calculator::CalculatorApp::new();
    let key = |k: &str| -> (u16,u16) { let rows=[["C","DEL","+/-","/"],["7","8","9","x"],["4","5","6","-"],["1","2","3","+"],["0",".","=","="]];
        for (r,row) in rows.iter().enumerate() { for (ci,kk) in row.iter().enumerate() { if *kk==k { return ((25+ci as i32*92+42) as u16, (140+r as i32*64+28) as u16); } } } (0,0) };
    for k in ["1","2",".","5","x","4","-","3","="] { let (x,y)=key(k); c.handle_tap(x,y); }
    let mut fb = Fb::new(); c.render(&mut fb); shot(&fb, "calculator");
    let mut l = crate::apps::level::LevelApp::new(); for _ in 0..20 { l.update(&inp(false,(0.10,-0.15,0.98))); }
    let mut fb = Fb::new(); l.render(&mut fb); shot(&fb, "level");
    let mut dd = crate::apps::dice::DiceApp::new(); dd.setup(); for _ in 0..30 { dd.update(&inp(false,(0.0,0.0,1.0))); }
    let mut fb = Fb::new(); dd.render(&mut fb); shot(&fb, "dice");
    let mut sm = crate::apps::sunmoon::SunMoonApp::new(); sm.set_date(2026,10,23,16);
    let mut fb = Fb::new(); sm.render(&mut fb); shot(&fb, "sun_moon");
    let mut q = crate::apps::wifiqr::WifiQrApp::new(); q.setup(); let mut fb = Fb::new(); q.render(&mut fb); shot(&fb, "wifi_qr");
    let mut fb = Fb::new(); crate::apps::smarthome::SmartHomeApp::new().render(&mut fb); shot(&fb, "smart_home");
    use crate::apps::ha_info::{HaInfoApp, HaKind};
    let mut h = HaInfoApp::new(HaKind::Status);
    h.set_result(200, "DTemperatures\nWBedroom: 71F\nWLiving Room: 70F\nWOutdoor: 58F\nDDoors & windows\nGFront Door: closed\nRGarage Door: OPEN\nGBack Door: closed\n", 10, 9);
    let mut fb = Fb::new(); h.render(&mut fb); shot(&fb, "home_status");
    let mut w = HaInfoApp::new(HaKind::Weather);
    w.set_result(200, "CPartly Cloudy\nB62 F\nWFeels like 60\nWHumidity 54%\nWWind 9 mph\nDForecast Home", 10, 9);
    let mut fb = Fb::new(); w.render(&mut fb); shot(&fb, "weather");
    let mut f = HaInfoApp::new(HaKind::Fleet);
    f.set_result(200, "GServer 1: online\nGServer 2: online\nRMini PC: OFFLINE\nDLaptop: no HA entity", 10, 9);
    let mut fb = Fb::new(); f.render(&mut fb); shot(&fb, "fleet");
    let mut wv = crate::apps::waves::WavesApp::new();
    for _ in 0..60 { wv.step((1.0,0.0,0.0), 0.04); } for _ in 0..12 { wv.step((0.866,0.5,0.0), 0.04); }
    let mut fb = Fb::new(); wv.render(&mut fb); shot(&fb, "waves");
    let mut bp = crate::apps::fluid::FluidApp::new(); for _ in 0..400 { bp.step((1.0,0.0,0.0)); } for _ in 0..60 { bp.step((0.866,0.5,0.0)); }
    let mut fb = Fb::new(); bp.render(&mut fb); shot(&fb, "ball_pit");
    let mut ast = crate::apps::asteroids::AsteroidsApp::new(); for i in 0..40 { ast.update(&inp(i % 10 == 0,(0.0,0.2,0.98))); }
    let mut fb = Fb::new(); ast.render(&mut fb); shot(&fb, "asteroids");
    let mut t = crate::apps::tictactoe::TicTacToeApp::new(); t.handle_tap(110,110); t.handle_tap(290,290);
    let mut fb = Fb::new(); t.render(&mut fb); shot(&fb, "tic_tac_toe");
    let mut te = crate::apps::tetris::TetrisGame::new(); te.setup(); for i in 0..2500 { te.update(&crate::apps::AppInput { touch: None, swipe: if i % 37 == 5 { Some(crate::peripherals::touch::SwipeDirection::Left) } else if i % 53 == 9 { Some(crate::peripherals::touch::SwipeDirection::Right) } else { None }, tap: i % 29 == 0, accel: (0.0,0.0,1.0), dt_ms: 50 }); }
    let mut fb = Fb::new(); te.render(&mut fb); shot(&fb, "tetris");
    let mut mz = crate::apps::maze::MazeGame::new(); mz.setup(); for _ in 0..60 { mz.update(&inp(false,(0.2,0.3,0.9))); }
    let mut fb = Fb::new(); mz.render(&mut fb); shot(&fb, "maze");
    use crate::apps::settings::*;
    let mut s = SettingsApp::new();
    s.set_status(WifiStatus::Connected { net: MODE_HOME, ip: Some([192,168,1,42]) });
    let mut fb = Fb::new(); s.render(&mut fb); shot(&fb, "settings_wifi");
    s.handle_tap(200, 240); s.set_status(WifiStatus::Connecting);
    let mut fb = Fb::new(); s.render(&mut fb); shot(&fb, "settings_hotspot");
}
