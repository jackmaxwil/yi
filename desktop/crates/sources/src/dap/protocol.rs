//! The debug-adapter messages this app sends and reads, hand-written over `serde_json`.

use std::io::{BufRead, Read};

use serde::Deserialize;
use serde_json::{Value, json};

/// The most one message's header block may take; real adapters send one 20-byte line.
pub const MAX_HEADER: usize = 1024;
/// The most one message's body may take: a huge string variable, not a stream of them.
pub const MAX_BODY: usize = 16 << 20;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("the adapter closed its output")]
    Closed,
    #[error("the adapter sent a header over {MAX_HEADER} bytes")]
    HeaderTooLong,
    #[error("the adapter sent a header with no Content-Length")]
    NoLength,
    #[error("the adapter sent a message of {len} bytes, over {MAX_BODY}")]
    TooLarge { len: usize },
    #[error("the adapter's output failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Reads one `Content-Length` framed body. Every error ends the stream: after a bad header
/// the reader is mid-message and nothing after it can be trusted to start a frame.
pub fn read_frame(reader: &mut impl BufRead) -> Result<Vec<u8>, FrameError> {
    let mut length = None;
    let mut used = 0usize;
    loop {
        let mut line = Vec::new();
        let room = MAX_HEADER.saturating_sub(used).saturating_add(1) as u64;
        let n = reader.by_ref().take(room).read_until(b'\n', &mut line)?;
        if n == 0 {
            return Err(FrameError::Closed);
        }
        used = used.saturating_add(n);
        if used > MAX_HEADER {
            return Err(FrameError::HeaderTooLong);
        }
        let line = line.trim_ascii();
        if line.is_empty() {
            break;
        }
        let header = std::str::from_utf8(line)
            .ok()
            .and_then(|l| l.split_once(':'));
        if let Some((name, value)) = header
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>().ok();
        }
    }
    let len = length.ok_or(FrameError::NoLength)?;
    if len > MAX_BODY {
        return Err(FrameError::TooLarge { len });
    }
    let mut body = Vec::new();
    reader.by_ref().take(len as u64).read_to_end(&mut body)?;
    if body.len() < len {
        return Err(FrameError::Closed);
    }
    Ok(body)
}

/// One message framed for the adapter's stdin.
pub fn frame(message: &Value) -> Vec<u8> {
    let body = message.to_string();
    let mut out = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

pub fn request(seq: u64, command: &str, arguments: Value) -> Value {
    json!({"seq": seq, "type": "request", "command": command, "arguments": arguments})
}

/// The answer to a request the adapter sent that this app does not take.
pub fn refusal(seq: u64, request_seq: u64, command: &str) -> Value {
    json!({
        "seq": seq, "type": "response", "request_seq": request_seq, "command": command,
        "success": false, "message": "not supported by this client",
    })
}

/// What the adapter sent, read loosely: unknown types, events and fields are kept as they are.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Response {
        request_seq: u64,
        success: bool,
        message: Option<String>,
        body: Value,
    },
    Event {
        event: String,
        body: Value,
    },
    Request {
        seq: u64,
        command: String,
    },
    Other,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    seq: u64,
    #[serde(default)]
    request_seq: u64,
    #[serde(default)]
    success: bool,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    command: String,
    #[serde(default)]
    event: String,
    #[serde(default)]
    body: Value,
}

/// A body that is not JSON or not an object is `None`: skipped, not fatal.
pub fn parse(bytes: &[u8]) -> Option<Message> {
    let e: Envelope = serde_json::from_slice(bytes).ok()?;
    Some(match e.kind.as_str() {
        "response" => Message::Response {
            request_seq: e.request_seq,
            success: e.success,
            message: e.message,
            body: e.body,
        },
        "event" => Message::Event {
            event: e.event,
            body: e.body,
        },
        "request" => Message::Request {
            seq: e.seq,
            command: e.command,
        },
        _ => Message::Other,
    })
}

/// A typed read of a body; a body of another shape reads as the type's default.
pub fn body<T: for<'de> Deserialize<'de> + Default>(body: &Value) -> T {
    T::deserialize(body).unwrap_or_default()
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Stopped {
    pub reason: String,
    pub description: Option<String>,
    pub thread_id: Option<u64>,
    pub hit_breakpoint_ids: Vec<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Process {
    pub name: String,
    pub system_process_id: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Output {
    pub category: Option<String>,
    pub output: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Exited {
    pub exit_code: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Threads {
    pub threads: Vec<Thread>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Thread {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Stack {
    pub stack_frames: Vec<Frame>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Frame {
    pub id: u64,
    pub name: String,
    pub source: Option<Source>,
    pub line: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Source {
    pub name: Option<String>,
    pub path: Option<String>,
    /// Non-zero for text the adapter makes up, such as disassembly: there is no file.
    pub source_reference: u64,
}

impl Source {
    pub fn file(&self) -> Option<String> {
        self.path.clone().filter(|_| self.source_reference == 0)
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Scopes {
    pub scopes: Vec<Scope>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Scope {
    pub name: String,
    pub presentation_hint: Option<String>,
    pub variables_reference: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Variables {
    pub variables: Vec<Variable>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Variable {
    pub name: String,
    pub value: String,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub variables_reference: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Evaluated {
    pub result: String,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub variables_reference: u64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Breakpoints {
    pub breakpoints: Vec<Breakpoint>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Breakpoint {
    pub id: Option<u64>,
    pub verified: bool,
    pub line: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Capabilities {
    pub supports_configuration_done_request: bool,
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::io::{BufReader, Cursor};

    use super::*;

    #[test]
    fn a_frame_reads_back_whatever_the_header_case_and_extra_headers() -> Result<(), Box<dyn Error>>
    {
        let wire = b"content-length: 2\r\nX-Other: 1\r\n\r\n{}Content-Length: 4\r\n\r\nnull";
        let mut reader = BufReader::with_capacity(3, Cursor::new(&wire[..]));
        assert_eq!(read_frame(&mut reader)?, b"{}");
        assert_eq!(read_frame(&mut reader)?, b"null");
        assert!(matches!(read_frame(&mut reader), Err(FrameError::Closed)));
        Ok(())
    }

    #[test]
    fn a_body_over_the_cap_is_refused_before_it_is_read() {
        let wire = format!("Content-Length: {}\r\n\r\n", MAX_BODY + 1);
        let got = read_frame(&mut Cursor::new(wire.into_bytes()));
        assert!(matches!(got, Err(FrameError::TooLarge { len }) if len == MAX_BODY + 1));
    }

    #[test]
    fn a_header_with_no_end_is_cut_at_the_cap() {
        let wire = vec![b'a'; MAX_HEADER * 4];
        let got = read_frame(&mut Cursor::new(wire));
        assert!(matches!(got, Err(FrameError::HeaderTooLong)));
    }

    #[test]
    fn a_body_cut_short_is_a_closed_stream() {
        let got = read_frame(&mut Cursor::new(b"Content-Length: 9\r\n\r\n{}".to_vec()));
        assert!(matches!(got, Err(FrameError::Closed)));
    }

    #[test]
    fn unknown_events_and_fields_are_read_not_refused() -> Result<(), Box<dyn Error>> {
        let got = parse(br#"{"seq":1,"type":"event","event":"brandNew","body":{"x":[1]},"y":2}"#);
        assert_eq!(
            got,
            Some(Message::Event {
                event: "brandNew".into(),
                body: json!({"x": [1]})
            })
        );
        let stopped: Stopped =
            body(&json!({"reason": "step", "threadId": 3, "allThreadsStopped": true}));
        assert_eq!(
            (stopped.reason.as_str(), stopped.thread_id),
            ("step", Some(3))
        );
        assert_eq!(parse(b"not json"), None);
        Ok(())
    }

    #[test]
    fn a_framed_request_carries_its_exact_length() -> Result<(), Box<dyn Error>> {
        let bytes = frame(&request(7, "threads", json!({"é": 1})));
        let body = read_frame(&mut Cursor::new(bytes))?;
        let back: Value = serde_json::from_slice(&body)?;
        assert_eq!(back["command"], "threads");
        Ok(())
    }
}
