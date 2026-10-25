//! A loopback HTTP endpoint answering a scripted (status, body) per request.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

pub type Res<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub type Seen = Arc<Mutex<Vec<String>>>;

/// Answers each request with the next scripted reply and closes; past the script the port
/// refuses. A request is recorded as its request line and form body; a string body goes raw.
pub fn serve(script: Vec<(u16, Value)>) -> Res<(u16, Seen)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let seen: Seen = Arc::default();
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for (status, body) in script {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            let head = loop {
                if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    break at + 4;
                }
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break request.len(),
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            };
            let text = String::from_utf8_lossy(&request[..head]).to_string();
            let length = text
                .lines()
                .find_map(|line| {
                    let line = line.to_ascii_lowercase();
                    line.strip_prefix("content-length:")?.trim().parse().ok()
                })
                .unwrap_or(0_usize);
            // A bare read here sat out the whole 5 s timeout whenever the body came with the
            // headers; closing with unread request bytes RSTs the response on macOS.
            while request.len() < head + length {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&buf[..n]),
                }
            }
            let line = text.lines().next().unwrap_or_default().to_owned();
            let form = String::from_utf8_lossy(&request[head..]).to_string();
            if let Ok(mut log) = log.lock() {
                log.push(format!("{line}\n{form}"));
            }
            let body = match body {
                Value::String(raw) => raw,
                json => json.to_string(),
            };
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
            let _ = stream.shutdown(std::net::Shutdown::Write);
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            while stream.read(&mut buf).map(|n| n > 0).unwrap_or(false) {}
        }
    });
    Ok((port, seen))
}
