use serde_json::{Map, Value, json};
use std::error::Error;
use yi_ai::faux::{FauxProvider, faux_assistant_message, faux_text, faux_thinking, faux_tool_call};
use yi_types::event::AssistantMessageEvent;
use yi_types::message::StopReason;

fn kinds(events: &[AssistantMessageEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            AssistantMessageEvent::Start { .. } => "start",
            AssistantMessageEvent::TextStart { .. } => "text_start",
            AssistantMessageEvent::TextDelta { .. } => "text_delta",
            AssistantMessageEvent::TextEnd { .. } => "text_end",
            AssistantMessageEvent::ThinkingStart { .. } => "thinking_start",
            AssistantMessageEvent::ThinkingDelta { .. } => "thinking_delta",
            AssistantMessageEvent::ThinkingEnd { .. } => "thinking_end",
            AssistantMessageEvent::ToolCallStart { .. } => "toolcall_start",
            AssistantMessageEvent::ToolCallDelta { .. } => "toolcall_delta",
            AssistantMessageEvent::ToolCallEnd { .. } => "toolcall_end",
            AssistantMessageEvent::Done { .. } => "done",
            AssistantMessageEvent::Error { .. } => "error",
        })
        .collect()
}

#[test]
fn scripted_message_replays_pi_event_order() -> Result<(), Box<dyn Error>> {
    let mut arguments = Map::new();
    arguments.insert("cmd".to_owned(), json!("ls"));
    let message = faux_assistant_message(
        vec![
            faux_thinking("think hard about it now"),
            faux_text("done"),
            faux_tool_call("call-1", "bash", arguments),
        ],
        StopReason::ToolUse,
    );
    let mut provider = FauxProvider::default();
    provider.set_responses(vec![message]);
    let events = provider.stream();
    assert_eq!(
        kinds(&events),
        [
            "start",
            "thinking_start",
            "thinking_delta",
            "thinking_delta",
            "thinking_end",
            "text_start",
            "text_delta",
            "text_end",
            "toolcall_start",
            "toolcall_delta",
            "toolcall_end",
            "done",
        ]
    );
    let wire: Value = serde_json::from_str(&serde_json::to_string(&events[0])?)?;
    assert_eq!(wire["type"], "start");
    assert_eq!(wire["partial"]["role"], "assistant");
    assert_eq!(wire["partial"]["stopReason"], "pending");
    let last = serde_json::to_value(events.last().ok_or("empty")?)?;
    assert_eq!(last["type"], "done");
    assert_eq!(last["reason"], "toolUse");
    Ok(())
}

#[test]
fn exhausted_queue_yields_error_event() -> Result<(), Box<dyn Error>> {
    let mut provider = FauxProvider::default();
    let events = provider.stream();
    let wire = serde_json::to_value(events.first().ok_or("empty")?)?;
    assert_eq!(wire["type"], "error");
    assert_eq!(wire["reason"], "error");
    assert_eq!(
        wire["error"]["errorMessage"],
        "No more faux responses queued"
    );
    assert_eq!(provider.call_count, 1);
    Ok(())
}
