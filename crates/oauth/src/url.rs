//! The small URL helpers both OAuth callers (provider login here, MCP login in
//! `yi-mcp-cli`) share: one copy, one set of rules.

/// application/x-www-form-urlencoded, unreserved set per RFC 3986 §2.3.
pub fn urlencode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

pub fn urldecode(text: &str) -> String {
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

pub fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| urldecode(value))
    })
}

/// The inverse of `pkce::base64url_nopad`, for reading a JWT payload segment.
pub fn base64url_decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => u32::from(byte - b'A'),
            b'a'..=b'z' => u32::from(byte - b'a') + 26,
            b'0'..=b'9' => u32::from(byte - b'0') + 52,
            b'-' => 62,
            b'_' => 63,
            b'=' => continue,
            _ => return None,
        };
        acc = (acc << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// Every endpoint a profile names must be https, or http to an exact loopback host —
/// a prefix check lets `http://localhost.attacker.example/token` ride off with the verifier.
pub fn https_or_local(url: &str) -> Result<(), String> {
    if url.starts_with("https://") {
        return Ok(());
    }
    let authority = url
        .strip_prefix("http://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .and_then(|authority| authority.rsplit('@').next())
        .unwrap_or("");
    let host = authority
        .strip_prefix('[')
        .and_then(|v6| v6.split(']').next())
        .unwrap_or_else(|| authority.split(':').next().unwrap_or(""));
    if host == "127.0.0.1" || host == "localhost" || host == "::1" {
        Ok(())
    } else {
        Err(format!("insecure endpoint refused: {url}"))
    }
}

#[cfg(test)]
mod tests {
    use super::https_or_local;

    #[test]
    fn plain_http_to_a_loopback_lookalike_is_refused() {
        assert!(https_or_local("https://example.com/token").is_ok());
        assert!(https_or_local("http://127.0.0.1:8317/token").is_ok());
        assert!(https_or_local("http://localhost/token").is_ok());
        assert!(https_or_local("http://[::1]:80/token").is_ok());
        assert!(https_or_local("http://localhost.attacker.example/token").is_err());
        assert!(https_or_local("http://127.0.0.1.evil.example/token").is_err());
        assert!(https_or_local("http://user@localhost.evil.example/token").is_err());
        assert!(https_or_local("http://example.com/token").is_err());
        assert!(https_or_local("ftp://127.0.0.1/token").is_err());
    }
}
