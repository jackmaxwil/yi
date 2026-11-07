//! A cache-field 400 is asked again once without the fields, and the reply records it (#748).

use std::error::Error;
use std::sync::atomic::Ordering;

use serde_json::json;
use yi_ai::breakpoints::{Encoded, Ttl};
use yi_ai::request::Stripped;
use yi_types::event::AssistantMessageEvent;
use yi_types::message::AgentMessage;
use yi_types::model::{LlmContext, Reuse};

use crate::common::model;
use crate::loopback_server::serve;

type TestResult = Result<(), Box<dyn Error>>;

fn refused(message: &str) -> String {
    let body = json!({"error": {"message": message}}).to_string();
    format!(
        "HTTP/1.1 400 Bad Request\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}

/// The same stream with the length stated, so the reader never waits on a close.
fn answer() -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{ANSWER}",
        ANSWER.len()
    )
}

/// Anthropic's Messages stream order, one text block and a stop.
const ANSWER: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","content":[],"model":"claude-opus-5.5","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":500,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Checked."}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":3}}

event: message_stop
data: {"type":"message_stop"}

"#;

fn marked_body() -> Encoded {
    let messages = json!([{"role": "user", "content": [{
        "type": "text",
        "text": "hi",
        "cache_control": {"type": "ephemeral"},
    }]}]);
    Encoded::provider_prefix(
        json!({
            "model": "probe",
            "messages": messages,
            "prompt_cache_key": "affinity",
            "provider": {"order": ["amazon-bedrock"]},
        }),
        "messages",
        Vec::new(),
    )
}

#[test]
fn stripping_keeps_fields_inside_tool_schemas_and_payloads() -> TestResult {
    let body = Encoded::provider_prefix(
        json!({
            "prompt_cache_key": "affinity",
            "tools": [{"input_schema": {
                "properties": {"session_id": {"type": "string"}, "cache": {"type": "boolean"}},
                "required": ["session_id", "cache"]
            }}],
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "hi", "cache_control": {"type": "ephemeral"}}
                ]},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "input": {"session_id": "keep", "cache_control": "keep"}}
                ]}
            ]
        }),
        "messages",
        Vec::new(),
    );
    let stripped = body
        .without(&["prompt_cache_key", "cache_control", "session_id", "cache"])
        .into_value();
    assert!(stripped.get("prompt_cache_key").is_none());
    assert_eq!(
        stripped.pointer("/messages/0/content/0/cache_control"),
        None,
        "the encoder's mark must be removed"
    );
    assert_eq!(
        stripped.pointer("/tools/0/input_schema/properties/session_id/type"),
        Some(&json!("string"))
    );
    assert_eq!(
        stripped.pointer("/tools/0/input_schema/properties/cache/type"),
        Some(&json!("boolean"))
    );
    assert_eq!(
        stripped.pointer("/messages/1/content/0/input/session_id"),
        Some(&json!("keep"))
    );
    assert_eq!(
        stripped.pointer("/messages/1/content/0/input/cache_control"),
        Some(&json!("keep"))
    );
    Ok(())
}

fn url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/v1/messages")
}

/// Dies with the field kept: a 400 naming `cache_control` resends the body without it.
#[test]
fn a_400_naming_a_cache_field_is_resent_once_without_it() -> TestResult {
    let (port, count, server) = serve(vec![refused("cache_control is not supported"), answer()])?;
    let stripped = Stripped::default();
    let quiet = |_| {};
    let retry = yi_ai::request::Retry::new(std::time::Duration::from_secs(60), None, &quiet);
    let answer = yi_ai::request::send_stripping_cache(
        &url(port),
        &[],
        &marked_body(),
        None,
        &retry,
        &stripped,
    )?;
    let _ = answer.into_string();
    let requests = server.join().map_err(|_| "server thread panicked")?;
    assert_eq!(
        count.load(Ordering::SeqCst),
        2,
        "the refusal was not resent"
    );
    assert!(requests[0].contains("cache_control"), "{}", requests[0]);
    assert!(
        !requests[1].contains("cache_control"),
        "the resend kept the field the route refused: {}",
        requests[1]
    );
    assert!(
        !requests[1].contains("prompt_cache_key") && !requests[1].contains("session_id"),
        "the resend kept an affinity key: {}",
        requests[1]
    );
    assert!(
        requests[1].contains("amazon-bedrock"),
        "the resend dropped more than the cache fields: {}",
        requests[1]
    );
    assert_eq!(stripped.take(), Some("cache_control"));
    Ok(())
}

/// Dies with a resend: a 400 naming no cache field is reported at once.
#[test]
fn a_400_that_names_no_cache_field_is_not_resent() -> TestResult {
    let (port, count, server) = serve(vec![refused("temperature is not supported")])?;
    let stripped = Stripped::default();
    let quiet = |_| {};
    let retry = yi_ai::request::Retry::new(std::time::Duration::from_secs(60), None, &quiet);
    let failed = yi_ai::request::send_stripping_cache(
        &url(port),
        &[],
        &marked_body(),
        None,
        &retry,
        &stripped,
    )
    .err()
    .ok_or("a refusal was reported as an answer")?;
    let _ = server.join().map_err(|_| "server thread panicked")?;
    assert_eq!(count.load(Ordering::SeqCst), 1, "{failed}");
    assert!(failed.starts_with("HTTP 400"), "{failed}");
    assert_eq!(stripped.take(), None);
    Ok(())
}

fn context() -> LlmContext {
    LlmContext {
        system_prompt: "You are a test.".to_owned(),
        messages: vec![AgentMessage::user_input(
            yi_types::message::UserContent::Text("hi".to_owned()),
            0,
        )],
        reuse: Reuse::Loop,
        transient: Vec::new(),
        schema: None,
        shared_through: None,
        cache_ttl: Ttl::Min5,
        cache_tail: true,
        tools: None,
        tool_choice: None,
    }
}

/// Dies with no note: the answered message records the field the route refused.
#[tokio::test]
async fn the_reply_records_the_field_the_route_refused() -> TestResult {
    let (port, count, server) = serve(vec![refused("cache_ttl must be omitted"), answer()])?;
    let model = model(
        "claude-opus-5.5",
        "anthropic-messages",
        "anthropic",
        &format!("http://127.0.0.1:{port}"),
        None,
    );
    let mut receiver = yi_ai::anthropic::stream(
        &model,
        &context(),
        &yi_ai::anthropic::AnthropicOptions::default(),
        "sk-test",
    );
    let mut answer = None;
    while let Some(event) = receiver.recv().await {
        match event {
            AssistantMessageEvent::Done { message, .. }
            | AssistantMessageEvent::Error { error: message, .. } => {
                answer = Some(message);
                break;
            }
            _ => {}
        }
    }
    let requests = server.join().map_err(|_| "server thread panicked")?;
    assert_eq!(
        count.load(Ordering::SeqCst),
        2,
        "the refusal was not resent: {requests:?}"
    );
    assert!(
        !requests[1].contains("cache_control"),
        "the resend kept the field: {}",
        requests[1]
    );
    let Some(AgentMessage::Assistant { diagnostics, .. }) = answer else {
        return Err("the stream ended without an assistant message".into());
    };
    let note = diagnostics
        .iter()
        .flatten()
        .find(|note| note.diagnostic_type == "cache_stripped")
        .ok_or_else(|| {
            format!("the reply carries no cache_stripped diagnostic: {diagnostics:?}")
        })?;
    assert_eq!(
        note.details
            .as_ref()
            .and_then(|details| details.get("field")),
        Some(&json!("cache_ttl")),
        "{note:?}"
    );
    Ok(())
}
