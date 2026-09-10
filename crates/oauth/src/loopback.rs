use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

const CALLBACK_TIMEOUT_SECS: u64 = 120;

pub struct Callback {
    pub code: String,
    pub state: String,
}

pub fn bind(port: u16, fallback: bool) -> Result<TcpListener, String> {
    let try_port = |port: u16| TcpListener::bind(("127.0.0.1", port));
    match try_port(port) {
        Ok(listener) => Ok(listener),
        Err(_error) if port != 0 && fallback => try_port(0).map_err(|error| error.to_string()),
        Err(error) => Err(format!("cannot bind loopback {port}: {error}")),
    }
}

pub fn redirect_uri(listener: &TcpListener, host: &str, path: &str) -> Result<String, String> {
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let path = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    Ok(format!("http://{host}:{port}{path}"))
}

fn urldecode(text: &str) -> String {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 3 <= bytes.len() => {
                let Some(hex) = text.get(index.saturating_add(1)..index.saturating_add(3)) else {
                    out.push(b'%');
                    index = index.saturating_add(1);
                    continue;
                };
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    index = index.saturating_add(3);
                    continue;
                }
                out.push(b'%');
                index = index.saturating_add(1);
            }
            b'+' => {
                out.push(b' ');
                index = index.saturating_add(1);
            }
            other => {
                out.push(other);
                index = index.saturating_add(1);
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| urldecode(value))
    })
}

pub fn wait_for_callback(listener: &TcpListener) -> Result<Callback, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let until = std::time::Instant::now() + Duration::from_secs(CALLBACK_TIMEOUT_SECS);
    let mut conn = loop {
        match listener.accept() {
            Ok((s, _)) => break s,
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= until {
                    return Err("OAuth callback timed out".to_owned());
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(err) => return Err(format!("callback accept failed: {err}")),
        }
    };
    conn.set_nonblocking(false).map_err(|e| e.to_string())?;
    let _ = conn.set_read_timeout(Some(Duration::from_secs(8)));
    let mut buf = [0u8; 8192];
    let n = conn.read(&mut buf).map_err(|e| e.to_string())?;
    let owned = String::from_utf8_lossy(buf.get(..n).unwrap_or(&[])).into_owned();
    let path = owned
        .lines()
        .next()
        .and_then(|row| row.split_whitespace().nth(1))
        .ok_or("malformed callback request")?;
    let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
    let code = query_param(query, "code").ok_or("callback missing code")?;
    let state = query_param(query, "state").ok_or("callback missing state")?;
    let _ = conn.write_all(
        b"HTTP/1.1 200 OK\r\ncontent-type: text/html\r\n\r\n<html><body>Login complete. Return to the terminal.</body></html>",
    );
    Ok(Callback { code, state })
}
