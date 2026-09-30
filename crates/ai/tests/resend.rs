//! Issue #256: a stream that died before its first byte is sent once more, and the
//! resend rides the message as a diagnostic.

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
    let n = |v: u64| serde_json::Number::from(v);
    yi_types::model::Model {
        id: "m".to_owned(),
        name: "m".to_owned(),
        api: "openai-completions".to_owned(),
        provider: "openrouter".to_owned(),
        base_url: "https://example.invalid".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: yi_types::model::ModelCost {
            input: n(0),
            output: n(0),
            cache_read: n(0),
            cache_write: n(0),
            tiers: None,
        },
        context_window: 1000,
        max_tokens: 100,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
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

/// Dies with `unknown: true` on an HTTP 402: every stream starts its usage unknown so a stream
/// cut before its usage chunk cannot read as free, but a refused request generated nothing.
/// The dogfood session's two `+?` marks were both 402 refusals that billed nothing.
#[test]
fn a_refused_request_bills_a_known_zero() -> TestResult {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || -> std::io::Result<()> {
        let (stream, _) = listener.accept()?;
        let mut reader = BufReader::new(stream);
        let mut length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 || line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        reader.read_exact(&mut vec![0; length])?;
        let body = r#"{"error":{"message":"Insufficient credits","code":402}}"#;
        let reply = format!(
            "HTTP/1.1 402 Payment Required\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        reader.get_mut().write_all(reply.as_bytes())
    });
    let probe = yi_ai::breakpoints::Encoded::provider_prefix(
        serde_json::json!({"model": "probe"}),
        "messages",
        Vec::new(),
    );
    let refused = yi_ai::request::send_with_retry(
        &format!("http://127.0.0.1:{port}/v1/chat/completions"),
        &[],
        &probe,
        None,
        &|_| {},
    )
    .err()
    .ok_or("a 402 was reported as success")?;
    server.join().map_err(|_| "server thread panicked")??;
    let usage_after = |text: &str| {
        let mut output = empty_assistant(&model());
        let _ = yi_ai::request::fail_message(&mut output, text);
        match output {
            AgentMessage::Assistant { usage, .. } => Some(usage.unknown),
            _ => None,
        }
    };
    assert_eq!(usage_after(&refused), Some(false), "{refused}");
    assert_eq!(
        usage_after("Stream ended without finish_reason"),
        Some(true),
        "a cut stream may have billed; its usage stays unknown"
    );
    Ok(())
}
