//! D145: a delta is a delta. The accumulator folds the stream into one message,
//! and the bytes on the wire stay linear in the answer.

use serde_json::Map;
use yi_types::event::{AssistantMessageEvent, apply};
use yi_types::message::{AgentMessage, Content, StopReason, Usage};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn assistant(content: Vec<Content>) -> Result<AgentMessage, serde_json::Error> {
    serde_json::from_value(serde_json::json!({
        "role": "assistant",
        "content": content,
        "api": "faux",
        "provider": "faux",
        "model": "faux-1",
        "usage": Usage::zero(),
        "stopReason": "pending",
        "timestamp": 0,
    }))
}

fn text_of(message: &AgentMessage) -> String {
    match message {
        AgentMessage::Assistant { content, .. } => content
            .iter()
            .filter_map(|block| match block {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect(),
        _ => String::new(),
    }
}

#[test]
fn deltas_fold_into_one_message_in_order() -> TestResult {
    let mut message: AgentMessage = serde_json::from_value(serde_json::json!({
        "role": "user",
        "content": "",
        "timestamp": 0,
    }))?;
    apply(
        &mut message,
        &AssistantMessageEvent::Start {
            partial: assistant(Vec::new())?,
        },
    );
    apply(
        &mut message,
        &AssistantMessageEvent::ThinkingStart { content_index: 0 },
    );
    apply(
        &mut message,
        &AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: "hm".to_owned(),
        },
    );
    apply(
        &mut message,
        &AssistantMessageEvent::TextStart { content_index: 1 },
    );
    for piece in ["one ", "two ", "three"] {
        apply(
            &mut message,
            &AssistantMessageEvent::TextDelta {
                content_index: 1,
                delta: piece.to_owned(),
            },
        );
    }
    apply(
        &mut message,
        &AssistantMessageEvent::TextEnd {
            content_index: 1,
            content: "one two three".to_owned(),
        },
    );
    apply(
        &mut message,
        &AssistantMessageEvent::ToolCallStart { content_index: 2 },
    );
    apply(
        &mut message,
        &AssistantMessageEvent::ToolCallDelta {
            content_index: 2,
            delta: "{\"pa".to_owned(),
        },
    );
    let call = Content::ToolCall {
        id: "call-1".to_owned(),
        name: "bash".to_owned(),
        arguments: Map::new(),
        thought_signature: None,
        namespace: None,
    };
    apply(
        &mut message,
        &AssistantMessageEvent::ToolCallEnd {
            content_index: 2,
            tool_call: call.clone(),
        },
    );
    assert_eq!(text_of(&message), "one two three");
    let AgentMessage::Assistant { content, .. } = &message else {
        return Err("not an assistant message".into());
    };
    assert_eq!(content.len(), 3, "{content:?}");
    assert!(matches!(&content[0], Content::Thinking { thinking, .. } if thinking == "hm"));
    assert_eq!(content[2], call);
    Ok(())
}

#[test]
fn a_start_past_the_end_still_lands_and_done_replaces_everything() -> TestResult {
    let mut message = assistant(Vec::new())?;
    apply(
        &mut message,
        &AssistantMessageEvent::TextStart { content_index: 2 },
    );
    apply(
        &mut message,
        &AssistantMessageEvent::TextDelta {
            content_index: 2,
            delta: "late".to_owned(),
        },
    );
    assert_eq!(text_of(&message), "late");
    let done = assistant(vec![Content::Text {
        text: "final".to_owned(),
        text_signature: None,
    }])?;
    apply(
        &mut message,
        &AssistantMessageEvent::Done {
            reason: StopReason::Stop,
            message: done.clone(),
        },
    );
    assert_eq!(message, done);
    Ok(())
}

#[test]
fn the_wire_is_linear_in_the_answer() -> TestResult {
    // 20,000 one-byte deltas: the old shape carried the whole partial in each,
    // 200 MB on the wire; a delta carries itself.
    let mut bytes = 0usize;
    for _ in 0..20_000 {
        bytes += serde_json::to_string(&AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "x".to_owned(),
        })?
        .len();
    }
    assert!(
        bytes < 20_000 * 64,
        "{bytes} bytes for 20,000 bytes of answer"
    );
    Ok(())
}
