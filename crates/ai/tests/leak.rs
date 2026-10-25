use std::error::Error;

use serde_json::{Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_ai::leak::{recover, recover_in};
use yi_ai::openai::ChunkMapper;
use yi_ai::sse::SseDecoder;
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, Content, StopReason};

type TestResult = Result<(), Box<dyn Error>>;

/// The exact text a glm-5.3-flash turn printed on 2026-09-14 in place of a call.
const LEAK: &str = "<tool_call>read<arg_key>path</arg_key><arg_value>~/.yi/skills/yi/plan/SKILL.md</arg_value></tool_call>";

#[test]
fn the_session_leak_is_one_read_call() -> TestResult {
    let (kept, calls) = recover(LEAK).ok_or("the markup spells a call")?;
    assert_eq!(kept, "");
    let (name, arguments) = calls.first().ok_or("one call")?;
    assert_eq!(name, "read");
    assert_eq!(
        arguments.get("path"),
        Some(&json!("~/.yi/skills/yi/plan/SKILL.md"))
    );
    assert_eq!(calls.len(), 1);
    Ok(())
}

#[test]
fn prose_around_a_block_survives_and_a_number_reads_as_one() -> TestResult {
    let text = "Reading it now. <tool_call>read<arg_key>path</arg_key><arg_value>a.rs</arg_value><arg_key>limit</arg_key><arg_value>120</arg_value></tool_call> Then the rest.";
    let (kept, calls) = recover(text).ok_or("a call")?;
    assert_eq!(
        kept.split_whitespace().collect::<Vec<_>>(),
        ["Reading", "it", "now.", "Then", "the", "rest."]
    );
    let (_, arguments) = calls.first().ok_or("one call")?;
    assert_eq!(arguments.get("limit"), Some(&json!(120)));
    assert_eq!(arguments.get("path"), Some(&json!("a.rs")));
    Ok(())
}

#[test]
fn an_unterminated_nameless_or_absent_block_is_text() {
    assert!(recover("<tool_call>read<arg_key>path").is_none());
    assert!(
        recover("<tool_call><arg_key>x</arg_key><arg_value>y</arg_value></tool_call>").is_none()
    );
    assert!(recover("prose that mentions no call").is_none());
}

#[test]
fn recover_in_turns_a_stop_into_a_tool_use_and_leaves_a_clean_message_alone() -> TestResult {
    let mut message = faux_assistant_message(vec![faux_text(LEAK)], StopReason::Stop);
    assert!(recover_in(&mut message));
    let AgentMessage::Assistant {
        content,
        stop_reason,
        ..
    } = &message
    else {
        return Err("assistant".into());
    };
    assert_eq!(*stop_reason, StopReason::ToolUse);
    assert!(matches!(content.first(), Some(Content::Text { text, .. }) if text.is_empty()));
    assert!(matches!(
        content.get(1),
        Some(Content::ToolCall { id, name, .. }) if id == "leak-0" && name == "read"
    ));
    let mut clean = faux_assistant_message(vec![faux_text("done")], StopReason::Stop);
    let before = clean.clone();
    assert!(!recover_in(&mut clean));
    assert_eq!(clean, before);
    Ok(())
}

/// The streamed call's arguments as the mapper's `Done` carries them, from an OpenAI-compatible
/// SSE body that sends `arguments` whole in one `tool_calls` delta.
fn streamed_arguments(arguments: &Value) -> Result<Value, Box<dyn Error>> {
    let call = json!({"id": "chatcmpl-1", "choices": [{"delta": {"tool_calls": [{"index": 0,
        "id": "call_1", "function": {"name": "todo", "arguments": arguments.to_string()}}]}}]});
    let stop =
        json!({"id": "chatcmpl-1", "choices": [{"delta": {}, "finish_reason": "tool_calls"}]});
    let model = crate::common::model("glm-5.3-flash", "openai-completions", "p", "http://h", None);
    let mut mapper = ChunkMapper::new(&model);
    let mut decoder = SseDecoder::default();
    for event in decoder.feed(&format!("data: {call}\n\ndata: {stop}\n\ndata: [DONE]\n\n")) {
        if let Ok(chunk) = serde_json::from_str::<Value>(&event.data) {
            drop(mapper.push_chunk(&chunk));
        }
    }
    match mapper.finish().pop() {
        Some(AssistantMessageEvent::Done {
            message: AgentMessage::Assistant { content, .. },
            ..
        }) => match content.into_iter().next() {
            Some(Content::ToolCall { arguments, .. }) => Ok(Value::Object(arguments)),
            other => Err(format!("expected the call: {other:?}").into()),
        },
        other => Err(format!("expected done: {other:?}").into()),
    }
}

/// Dies with a tool reading GLM's leaked markup as a field: a synthetic replay of the sweep's two
/// leaked `done` shapes reaches the tool with `evidence` as a field and the op word as a flag.
#[test]
fn a_streamed_call_with_leaked_markup_reaches_the_tool_repaired() -> TestResult {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../runtime/tests/fixtures/todo/glm-leaked-calls.json"
    ))?;
    for case in fixture["lands"].as_array().ok_or("lands")? {
        let arguments = streamed_arguments(&case["arguments"])?;
        let evidence = arguments["evidence"].as_str().map(str::trim);
        assert_eq!(evidence, case["evidence"].as_str(), "{arguments}");
        assert_eq!(
            arguments["label"], case["arguments"]["label"],
            "{arguments}"
        );
        assert_eq!(arguments["done"], json!(true), "{arguments}");
        assert_eq!(
            arguments.as_object().map(|map| map.len()),
            Some(3),
            "{arguments}"
        );
    }
    let clean = json!({"op": "done", "label": "a", "evidence": "x</arg_value> stays"});
    assert_eq!(streamed_arguments(&clean)?, clean);
    let prose = &fixture["refused"][3];
    assert_eq!(
        streamed_arguments(prose)?,
        *prose,
        "prose before the markup is no field"
    );
    Ok(())
}
