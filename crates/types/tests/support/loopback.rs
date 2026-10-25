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
