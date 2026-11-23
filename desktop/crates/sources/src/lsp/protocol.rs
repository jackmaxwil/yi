//! The language-server messages this app sends and reads, hand-written over `serde_json`:
//! `Content-Length` framing, message kinds, file uris and UTF-16 positions.

use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The longest header line read; a server's headers are two short lines.
pub const MAX_HEADER: usize = 4096;
/// Header lines one message may carry before its blank line.
pub const MAX_HEADERS: usize = 16;
/// The largest body read: the cap the wire to Yi's server uses.
pub const MAX_BODY: usize = 32 << 20;

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("the server closed its output")]
    Eof,
    #[error("reading from the server failed: {0}")]
    Io(#[from] io::Error),
    #[error("a header line ran past {MAX_HEADER} bytes")]
    HeaderTooLong,
    #[error("a message carried more than {MAX_HEADERS} header lines")]
    TooManyHeaders,
    #[error("a message had no Content-Length")]
    NoLength,
    #[error("a message of {len} bytes is past the {MAX_BODY}-byte cap")]
    TooLarge { len: u64 },
    #[error("a message body was not JSON")]
    NotJson,
}

/// One message off `from`: its headers, then exactly its body. Nothing past a cap is
/// allocated; a body that is not JSON is consumed whole, so the stream stays in step.
pub fn read_message(from: &mut impl BufRead) -> Result<Value, ReadError> {
    let mut length = None;
    let mut line = Vec::new();
    for lines in 0.. {
        if lines >= MAX_HEADERS {
            return Err(ReadError::TooManyHeaders);
        }
        line.clear();
        let cap = u64::try_from(MAX_HEADER).unwrap_or(u64::MAX);
        let got = from.by_ref().take(cap).read_until(b'\n', &mut line)?;
        if got == 0 {
            return Err(ReadError::Eof);
        }
        if line.last() != Some(&b'\n') {
            return Err(if got < MAX_HEADER {
                ReadError::Eof
            } else {
                ReadError::HeaderTooLong
            });
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim();
        if text.is_empty() {
            break;
        }
        if let Some((name, value)) = text.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = Some(
                value
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| ReadError::NoLength)?,
            );
        }
    }
    let len = length.ok_or(ReadError::NoLength)?;
    if len > u64::try_from(MAX_BODY).unwrap_or(u64::MAX) {
        return Err(ReadError::TooLarge { len });
    }
    let mut body = Vec::new();
    from.by_ref().take(len).read_to_end(&mut body)?;
    if u64::try_from(body.len()).ok() != Some(len) {
        return Err(ReadError::Eof);
    }
    serde_json::from_slice(&body).map_err(|_| ReadError::NotJson)
}

/// `message` framed with its `Content-Length`.
pub fn frame(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

pub fn write_message(to: &mut impl Write, message: &Value) -> io::Result<()> {
    to.write_all(&frame(message))?;
    to.flush()
}

pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

pub fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

pub fn answer(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// What a message off the server is. Fields this app does not read stay in the value.
#[derive(Debug, PartialEq)]
pub enum Incoming {
    /// The answer to one of this app's requests: its result or the error's message.
    Answer {
        id: u64,
        result: Result<Value, String>,
    },
    /// The server asks something of the app.
    Ask {
        id: Value,
        method: String,
        params: Value,
    },
    Note {
        method: String,
        params: Value,
    },
    /// Not a shape JSON-RPC has, or an answer to an id this app never used.
    Other,
}

pub fn classify(mut message: Value) -> Incoming {
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let params = message
        .get_mut("params")
        .map(Value::take)
        .unwrap_or(Value::Null);
    let id = message.get_mut("id").map(Value::take);
    match (id, method) {
        (Some(id), Some(method)) => Incoming::Ask { id, method, params },
        (None, Some(method)) => Incoming::Note { method, params },
        (Some(id), None) => {
            let Some(id) = id.as_u64() else {
                return Incoming::Other;
            };
            let result = match message.get_mut("error") {
                Some(error) => Err(error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("the server answered with an error")
                    .to_owned()),
                None => Ok(message
                    .get_mut("result")
                    .map(Value::take)
                    .unwrap_or_default()),
            };
            Incoming::Answer { id, result }
        }
        (None, None) => Incoming::Other,
    }
}

/// The `file://` uri of an absolute path, with every byte outside the unreserved set escaped.
pub fn uri_of(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

/// The path a `file://` uri names; none for another scheme or an escape that is not UTF-8.
pub fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(rest.len());
    let mut it = rest.bytes();
    while let Some(byte) = it.next() {
        if byte == b'%' {
            let hex = [it.next()?, it.next()?];
            let hex = std::str::from_utf8(&hex).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

/// The UTF-16 offset of character column `col` (from 0) on `line`; past the end, its end.
pub fn utf16_of(line: &str, col: usize) -> u64 {
    line.chars()
        .take(col)
        .map(|c| u64::try_from(c.len_utf16()).unwrap_or(2))
        .fold(0u64, u64::saturating_add)
}

/// The character column (from 0) of UTF-16 offset `at` on `line`. An offset inside a
/// surrogate pair counts its character as passed; past the end, the line's length.
pub fn char_of(line: &str, at: u64) -> usize {
    let mut units = 0u64;
    let mut col = 0usize;
    for c in line.chars() {
        if units >= at {
            break;
        }
        units = units.saturating_add(u64::try_from(c.len_utf16()).unwrap_or(2));
        col = col.saturating_add(1);
    }
    col
}

/// The byte offset in `line` of UTF-16 offset `at`, on a character boundary.
pub fn byte_of(line: &str, at: u64) -> usize {
    let col = char_of(line, at);
    line.char_indices()
        .nth(col)
        .map_or(line.len(), |(at, _)| at)
}

/// A protocol position `{line, character}` as numbers.
pub fn position(value: &Value) -> Option<(u64, u64)> {
    Some((
        value.get("line")?.as_u64()?,
        value.get("character")?.as_u64()?,
    ))
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io::Cursor;

    use super::*;

    const LINE: &str = "    let café = \"🦀\"; let x = 1;";

    #[test]
    fn a_character_column_past_an_astral_character_is_one_unit_further_in_utf16()
    -> Result<(), Box<dyn Error>> {
        assert_eq!(utf16_of(LINE, 24), 25);
        assert_eq!(char_of(LINE, 25), 24);
        assert_eq!(utf16_of(LINE, 11), 11);
        assert_eq!(char_of(LINE, 11), 11);
        assert_eq!(char_of(LINE, 99), LINE.chars().count());
        assert_eq!(LINE.get(byte_of(LINE, 25)..), Some("x = 1;"));
        Ok(())
    }

    #[test]
    fn two_messages_in_one_read_come_out_as_two() -> Result<(), Box<dyn Error>> {
        let mut bytes = frame(&json!({"id": 1, "result": null}));
        bytes.extend(frame(&json!({"method": "x", "params": {"é": 1}})));
        let mut from = Cursor::new(bytes);
        assert_eq!(
            classify(read_message(&mut from)?),
            Incoming::Answer {
                id: 1,
                result: Ok(Value::Null)
            }
        );
        let second = classify(read_message(&mut from)?);
        assert_eq!(
            second,
            Incoming::Note {
                method: "x".into(),
                params: json!({"é": 1})
            }
        );
        assert!(matches!(read_message(&mut from), Err(ReadError::Eof)));
        Ok(())
    }

    #[test]
    fn a_length_past_the_cap_is_refused_before_it_is_read() -> Result<(), Box<dyn Error>> {
        let mut from = Cursor::new(b"Content-Length: 999999999999\r\n\r\n{}".to_vec());
        let got = read_message(&mut from);
        assert!(
            matches!(
                got,
                Err(ReadError::TooLarge {
                    len: 999_999_999_999
                })
            ),
            "{got:?}"
        );
        let mut long = vec![b'X'; MAX_HEADER + 10];
        long.push(b'\n');
        let got = read_message(&mut Cursor::new(long));
        assert!(matches!(got, Err(ReadError::HeaderTooLong)), "{got:?}");
        Ok(())
    }

    #[test]
    fn a_path_with_spaces_and_accents_round_trips_through_its_uri() -> Result<(), Box<dyn Error>> {
        let path = Path::new("/tmp/a b/café.rs");
        let uri = uri_of(path);
        assert_eq!(uri, "file:///tmp/a%20b/caf%C3%A9.rs"); // codespell:ignore caf
        assert_eq!(path_of(&uri).as_deref(), Some(path));
        Ok(())
    }
}
