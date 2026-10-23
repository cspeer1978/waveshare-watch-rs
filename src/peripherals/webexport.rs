// Tiny one-shot web server for exporting the battery log over Wi-Fi.
//
// While the Battery app's Wi-Fi export is active, the watch listens on port 80.
//   http://<watch-ip>/             -> the log as plain text (view / copy in the browser)
//   http://<watch-ip>/battery.csv  -> the same, as a file download
// Anything else (e.g. the browser's favicon request) gets a 404.
// Each call handles exactly one connection; main calls it in a loop until the
// user presses BOOT or the time runs out.

use alloc::vec;
use embassy_net::tcp::{State, TcpSocket};
use embassy_net::Stack;
use embassy_time::{Duration, Timer};

use crate::apps::battery::{csv_line, CSV_HEADER};
use crate::peripherals::battlog::Entry;

pub enum Served {
    Csv,   // sent the log
    Other, // answered some other request (404)
    Error, // connection failed
}

async fn write_all(s: &mut TcpSocket<'_>, mut buf: &[u8]) -> Result<(), ()> {
    while !buf.is_empty() {
        match s.write(buf).await {
            Ok(0) | Err(_) => return Err(()),
            Ok(n) => buf = &buf[n..],
        }
    }
    Ok(())
}

/// Wait for one HTTP request on port 80 and answer it.
pub async fn serve_one(stack: Stack<'_>, entries: &[Entry]) -> Served {
    // Buffers on the heap: this future lives inside the main task, and every
    // byte here would otherwise be taken from the (small) CPU stack.
    let mut rx = vec![0u8; 1024];
    let mut tx = vec![0u8; 2048];
    let mut req = vec![0u8; 512];
    let mut s = TcpSocket::new(stack, &mut rx, &mut tx);
    if s.accept(80).await.is_err() {
        return Served::Error;
    }
    s.set_timeout(Some(Duration::from_secs(10)));

    // Read the request head (we only need the path).
    let mut n = 0;
    loop {
        match s.read(&mut req[n..]).await {
            Ok(0) | Err(_) => break,
            Ok(k) => {
                n += k;
                if n == req.len() || req[..n].windows(4).any(|w| w == b"\r\n\r\n") { break; }
            }
        }
    }
    let head = core::str::from_utf8(&req[..n]).unwrap_or("");
    let path = head.split_whitespace().nth(1).unwrap_or("/");

    let result = if path == "/" || path.starts_with("/battery") {
        let hdr: &[u8] = if path.starts_with("/battery.csv") {
            b"HTTP/1.1 200 OK\r\nContent-Type: text/csv\r\nContent-Disposition: attachment; filename=\"watch-battery.csv\"\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
        } else {
            b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
        };
        let mut ok = write_all(&mut s, hdr).await.is_ok()
            && write_all(&mut s, CSV_HEADER.as_bytes()).await.is_ok()
            && write_all(&mut s, b"\n").await.is_ok();
        for e in entries {
            if !ok { break; }
            let line = csv_line(e);
            ok = write_all(&mut s, line.as_str().as_bytes()).await.is_ok()
                && write_all(&mut s, b"\n").await.is_ok();
        }
        if ok { Served::Csv } else { Served::Error }
    } else {
        let _ = write_all(&mut s, b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
        Served::Other
    };

    // Make sure everything actually leaves before the socket is dropped
    // (dropping it early would cut the download short).
    let _ = s.flush().await;
    s.close();
    let _ = embassy_time::with_timeout(Duration::from_secs(2), async {
        while !matches!(s.state(), State::Closed | State::TimeWait) {
            Timer::after(Duration::from_millis(20)).await;
        }
    })
    .await;
    result
}
