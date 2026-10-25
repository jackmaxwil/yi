//! A loopback HTTP endpoint answering a scripted (status, body) per request.

use std::io::{BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;

#[path = "../../../types/tests/support/loopback.rs"]
mod loopback;
use loopback::read_request;

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
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut reader = BufReader::new(stream);
            let request = read_request(&mut reader);
            let mut stream = reader.into_inner();
            let line = request.lines().next().unwrap_or_default();
            let form = request.rsplit("\r\n").next().unwrap_or_default();
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
            while stream
                .read(&mut [0u8; 4096])
                .map(|n| n > 0)
                .unwrap_or(false)
            {}
        }
    });
    Ok((port, seen))
}
