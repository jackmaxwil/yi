//! Issue #256: a stream that died before its first byte is sent once more, and the
//! resend rides the message as a diagnostic.

use crate::common;

use yi_ai::request::{empty_assistant, note_resend, resend_dead_stream};
use yi_types::message::AgentMessage;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn only_a_stream_that_showed_nothing_is_resent_and_only_once() {
    assert!(
        resend_dead_stream(false, false),
        "nothing delivered, first death"
    );
    assert!(
        !resend_dead_stream(true, false),
        "a delta was seen: a replay would duplicate it"
    );
    assert!(!resend_dead_stream(false, true), "already resent once");
    assert!(!resend_dead_stream(true, true));
}

fn model() -> yi_types::model::Model {
    common::model(
        "m",
        "openai-completions",
        "openrouter",
        "https://example.invalid",
        None,
    )
}

#[test]
fn the_resend_is_a_diagnostic_on_the_message() -> TestResult {
    let mut output = empty_assistant(&model());
    note_resend(&mut output, "Bad address (os error 14)");
    let AgentMessage::Assistant { diagnostics, .. } = &output else {
        return Err("not an assistant message".into());
    };
    let diagnostic = diagnostics
        .as_ref()
        .and_then(|list| list.first())
        .ok_or("no diagnostic recorded")?;
    assert_eq!(diagnostic.diagnostic_type, "stream_resent");
    assert_eq!(
        diagnostic.error.as_ref().map(|e| e.message.as_str()),
        Some("Bad address (os error 14)")
    );
    let wire = serde_json::to_value(&output)?;
    assert_eq!(wire["diagnostics"][0]["details"]["resends"], 1);
    Ok(())
}

/// Answers one request per reply in order, reading each request whole first.
fn serve(replies: Vec<&'static str>) -> std::io::Result<(u16, std::thread::JoinHandle<()>)> {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let handle = std::thread::spawn(move || {
        for reply in replies {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream);
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let _ = reader.read_exact(&mut vec![0; length]);
            let _ = reader.get_mut().write_all(reply.as_bytes());
        }
    });
    Ok((port, handle))
}

const REFUSED_402: &str = "HTTP/1.1 402 Payment Required\r\ncontent-type: application/json\r\ncontent-length: 55\r\n\r\n{\"error\":{\"message\":\"Insufficient credits\",\"code\":402}}";

fn probe() -> yi_ai::breakpoints::Encoded {
    yi_ai::breakpoints::Encoded::provider_prefix(
        serde_json::json!({"model": "probe"}),
        "messages",
        Vec::new(),
    )
}

fn unknown_after(text: &str) -> Option<bool> {
    let mut output = empty_assistant(&model());
    let _ = yi_ai::request::fail_message(&mut output, text);
    match output {
        AgentMessage::Assistant { usage, .. } => Some(usage.unknown),
        _ => None,
    }
}

/// Dies with `unknown: true` on an HTTP 402: every stream starts its usage unknown so a stream
/// cut before its usage chunk cannot read as free, but a 4xx refused the request before it ran.
/// The dogfood session's two `+?` marks were both 402 refusals that billed nothing. A 5xx may
/// come from a gateway after the upstream billed, so it stays unknown.
#[test]
fn a_refused_request_bills_a_known_zero() -> TestResult {
    let (port, server) = serve(vec![REFUSED_402])?;
    let refused = yi_ai::request::send_with_retry(
        &format!("http://127.0.0.1:{port}/v1/chat/completions"),
        &[],
        &probe(),
        None,
        &|_| {},
    )
    .err()
    .ok_or("a 402 was reported as success")?;
    server.join().map_err(|_| "server thread panicked")?;
    assert_eq!(unknown_after(&refused), Some(false), "{refused}");
    assert_eq!(
        unknown_after("HTTP 502: <html>Bad Gateway</html>"),
        Some(true),
        "a gateway 5xx may follow a billed upstream"
    );
    assert_eq!(
        unknown_after("Stream ended without finish_reason"),
        Some(true),
        "a cut stream may have billed; its usage stays unknown"
    );
    Ok(())
}

/// Dies with a resend's bare `HTTP 402`: a first 200 stream died before its first event, and
/// that request may have billed, so the resend's refusal must not read as a known zero.
#[test]
fn a_refused_resend_keeps_the_first_attempt_unknown() -> TestResult {
    let died = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: 64\r\n\r\n";
    let (port, server) = serve(vec![died, REFUSED_402])?;
    let url = format!("http://127.0.0.1:{port}/v1/chat/completions");
    let body = probe();
    let failed = yi_ai::request::pump_sse_with_resend(
        None,
        || yi_ai::request::send_with_retry(&url, &[], &body, None, &|_| {}),
        |_| Ok(true),
    )
    .err()
    .ok_or("a died stream and a refused resend were reported as success")?;
    server.join().map_err(|_| "server thread panicked")?;
    assert!(failed.contains("HTTP 402"), "{failed}");
    assert_eq!(unknown_after(&failed), Some(true), "{failed}");
    Ok(())
}
