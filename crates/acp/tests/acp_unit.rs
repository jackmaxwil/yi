use std::error::Error;

use serde_json::{Map, Value, json};
use yi_acp::update::{IdMap, base64, replay_updates, to_updates};
use yi_acp::{VERSION_MISMATCH_ERROR, negotiate};
use yi_types::acp::{AcpSessionUpdate, AcpState, AcpStopReason, AcpToolCallStatus};
use yi_types::entry::Entry;
use yi_types::event::{AgentEvent, AssistantMessageEvent, ToolResult};
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};

type TestResult = Result<(), Box<dyn Error>>;

fn assistant_partial() -> AgentMessage {
    AgentMessage::Assistant {
        content: Vec::new(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        model: "faux-1".to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: yi_types::message::Usage::zero(),
        stop_reason: StopReason::Stop,
        raw_stop_reason: None,
        end_turn: None,
        deferred: None,
        error_message: None,
        timestamp: 0,
    }
}

#[test]
fn negotiate_serves_v2_and_rejects_v1_with_the_exact_error() -> TestResult {
    assert_eq!(negotiate(2)?, 2);
    assert_eq!(negotiate(3)?, 2, "later versions are served as v2");
    let error = negotiate(1).err().ok_or("v1 must be rejected")?;
    assert_eq!(error, VERSION_MISMATCH_ERROR);
    Ok(())
}

#[test]
fn text_deltas_become_message_chunks_with_a_stable_message_id() -> TestResult {
    let mut ids = IdMap::new(1000);
    to_updates(
        &AgentEvent::MessageStart {
            message: assistant_partial(),
        },
        &mut ids,
    );
    let delta = |text: &str| AgentEvent::MessageUpdate {
        message: assistant_partial(),
        assistant_message_event: AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: text.to_owned(),
            partial: assistant_partial(),
        },
    };
    let first = to_updates(&delta("hel"), &mut ids);
    let second = to_updates(&delta("lo"), &mut ids);
    let id_of = |updates: &[AcpSessionUpdate]| match updates {
        [AcpSessionUpdate::AgentMessageChunk { message_id, .. }] => Ok(message_id.clone()),
        other => Err(format!("expected one chunk, got {other:?}")),
    };
    assert_eq!(
        id_of(&first)?,
        id_of(&second)?,
        "chunks share one messageId"
    );
    let json = serde_json::to_value(&first[0])?;
    assert_eq!(json["sessionUpdate"], "agent_message_chunk");
    assert_eq!(json["content"]["type"], "text");
    assert_eq!(json["content"]["text"], "hel");
    Ok(())
}

#[test]
fn agent_lifecycle_maps_to_state_updates_with_stop_reason() -> TestResult {
    let mut ids = IdMap::new(1000);
    let start = to_updates(&AgentEvent::AgentStart, &mut ids);
    assert_eq!(
        start,
        vec![AcpSessionUpdate::StateUpdate(AcpState::Running)]
    );
    to_updates(
        &AgentEvent::MessageUpdate {
            message: assistant_partial(),
            assistant_message_event: AssistantMessageEvent::Done {
                reason: StopReason::Aborted,
                message: assistant_partial(),
            },
        },
        &mut ids,
    );
    let end = to_updates(
        &AgentEvent::AgentEnd {
            messages: Vec::new(),
        },
        &mut ids,
    );
    assert_eq!(
        end,
        vec![AcpSessionUpdate::StateUpdate(AcpState::Idle {
            stop_reason: Some(AcpStopReason::Cancelled),
        })],
        "an aborted run must surface stopReason cancelled"
    );
    let json = serde_json::to_value(&end[0])?;
    assert_eq!(json["sessionUpdate"], "state_update");
    assert_eq!(json["state"], "idle");
    assert_eq!(json["stopReason"], "cancelled");
    Ok(())
}

#[test]
fn tool_execution_maps_to_tool_call_updates_and_bash_to_terminals() -> TestResult {
    let mut ids = IdMap::new(1000);
    let mut args = Map::new();
    args.insert("command".to_owned(), json!("ls"));
    let start = to_updates(
        &AgentEvent::ToolExecutionStart {
            tool_call_id: "t1".to_owned(),
            tool_name: "bash".to_owned(),
            args: Value::Object(args),
        },
        &mut ids,
    );
    let started = serde_json::to_value(&start[0])?;
    assert_eq!(started["sessionUpdate"], "tool_call_update");
    assert_eq!(started["status"], "in_progress");
    assert_eq!(started["kind"], "execute");
    assert_eq!(started["rawInput"]["command"], "ls");

    let end = to_updates(
        &AgentEvent::ToolExecutionEnd {
            tool_call_id: "t1".to_owned(),
            tool_name: "bash".to_owned(),
            result: ToolResult {
                content: vec![Content::Text {
                    text: "ok".to_owned(),
                    text_signature: None,
                }],
                details: json!({"exitCode": 0}),
                usage: None,
                added_tool_names: None,
                terminate: None,
            },
            is_error: false,
        },
        &mut ids,
    );
    let kinds: Vec<String> = end
        .iter()
        .map(|update| {
            serde_json::to_value(update).map(|json| {
                json["sessionUpdate"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
        })
        .collect::<Result<_, _>>()?;
    assert_eq!(
        kinds,
        vec![
            "terminal_output_chunk".to_owned(),
            "terminal_update".to_owned(),
            "tool_call_update".to_owned(),
        ],
        "bash results stream through the terminal vocabulary (C7)"
    );
    let chunk = serde_json::to_value(&end[0])?;
    assert_eq!(chunk["data"], base64(b"ok"));
    match &end[2] {
        AcpSessionUpdate::ToolCallUpdate { status, .. } => {
            assert_eq!(*status, Some(AcpToolCallStatus::Completed));
        }
        other => return Err(format!("expected tool_call_update, got {other:?}").into()),
    }
    Ok(())
}

#[test]
fn custom_messages_become_yi_extension_updates() -> TestResult {
    let mut ids = IdMap::new(1000);
    let updates = to_updates(
        &AgentEvent::MessageStart {
            message: AgentMessage::Custom {
                custom_type: "advisory".to_owned(),
                content: UserContent::Text("<advisory/>".to_owned()),
                display: true,
                details: None,
                timestamp: 0,
            },
        },
        &mut ids,
    );
    let json = serde_json::to_value(&updates[0])?;
    assert_eq!(
        json["sessionUpdate"], "_yi/advisory",
        "custom types map onto the _yi/ extension prefix (C9)"
    );
    assert_eq!(json["text"], "<advisory/>");
    Ok(())
}

#[test]
fn base64_pads_correctly() {
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foo"), "Zm9v");
    assert_eq!(base64(b"foob"), "Zm9vYg==");
}

#[test]
fn replay_walks_entries_into_full_message_updates() -> TestResult {
    let entries = vec![
        Entry::Message {
            id: "e1".to_owned(),
            message: AgentMessage::User {
                content: UserContent::Text("fix the bug".to_owned()),
                timestamp: 0,
            },
            terminate: None,
            parent_id: None,
            seq: 1,
            timestamp: 0,
        },
        Entry::Compaction {
            id: "e2".to_owned(),
            summary: "earlier work".to_owned(),
            retained_tail: Vec::new(),
            tokens_before: 10,
            details: None,
            usage: None,
            parent_id: Some("e1".to_owned()),
            seq: 2,
            timestamp: 0,
        },
    ];
    let mut ids = IdMap::new(1000);
    let updates = replay_updates(&entries, &mut ids);
    let first = serde_json::to_value(&updates[0])?;
    assert_eq!(first["sessionUpdate"], "user_message");
    assert_eq!(first["content"][0]["text"], "fix the bug");
    let second = serde_json::to_value(&updates[1])?;
    assert_eq!(second["sessionUpdate"], "_yi/compaction");
    assert_eq!(second["summary"], "earlier work");
    Ok(())
}

#[test]
fn permission_bridge_writes_the_request_and_maps_the_selected_outcome() -> TestResult {
    use std::sync::{Arc, Mutex};
    let pending: Arc<Mutex<std::collections::HashMap<String, std::sync::mpsc::Sender<Value>>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));
    let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let route = Arc::clone(&pending);
    let record = Arc::clone(&seen);
    let sink: yi_acp::LineSink = Arc::new(move |frame: &Value| {
        if let Ok(mut log) = record.lock() {
            log.push(frame.clone());
        }
        let Some(id) = frame.get("id").and_then(Value::as_str) else {
            return;
        };
        let sender = route.lock().ok().and_then(|map| map.get(id).cloned());
        if let Some(sender) = sender {
            let _ = sender.send(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"outcome": {"outcome": "selected", "optionId": "allow_always"}},
            }));
        }
    });
    let asker = yi_acp::bridge_asker("s1".to_owned(), sink, Arc::clone(&pending));
    let outcome = asker("bash requires permission", "rm -rf build");
    assert!(
        matches!(outcome, yi_runtime::AskOutcome::AllowAlways),
        "a selected allow_always option must map onto AskOutcome::AllowAlways"
    );
    let log = seen.lock().map_err(|error| error.to_string())?;
    let request = log
        .first()
        .ok_or("the request must be written before blocking")?;
    assert_eq!(request["method"], "session/request_permission");
    assert_eq!(request["params"]["sessionId"], "s1");
    assert_eq!(request["params"]["title"], "bash requires permission");
    assert_eq!(request["params"]["options"][0]["kind"], "allow_once");
    assert!(
        pending
            .lock()
            .map_err(|error| error.to_string())?
            .is_empty(),
        "the pending entry must be cleaned up after the answer"
    );
    Ok(())
}
