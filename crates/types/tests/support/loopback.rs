//! The one request reader every loopback HTTP fake shares, so a framing fix lands once.
use std::io::{BufRead, BufReader, Read};
use std::net::TcpStream;

/// One HTTP/1.1 request whole: request line, headers, then the `content-length` body.
pub fn read_request(reader: &mut BufReader<TcpStream>) -> String {
    let mut request = String::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
        request.push_str(&line);
    }
    let mut body = vec![0; length];
    let _ = reader.read_exact(&mut body);
    request + &String::from_utf8_lossy(&body)
}

/// The JSON body of a request [`read_request`] captured; it drops the blank line, so the
/// object opens on the line after the last header.
#[allow(dead_code, reason = "the shared harness serves several test binaries")]
pub fn json_body(request: &str) -> Result<serde_json::Value, String> {
    let (_, body) = request
        .split_once("\r\n{")
        .ok_or("request has no JSON body")?;
    serde_json::from_str(&format!("{{{body}")).map_err(|error| error.to_string())
}
