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

#[test]
fn the_resend_is_a_diagnostic_on_the_message() -> TestResult {
    let n = |v: u64| serde_json::Number::from(v);
    let model = yi_types::model::Model {
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
    };
    let mut output = empty_assistant(&model);
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
