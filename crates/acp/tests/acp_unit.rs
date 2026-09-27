use std::error::Error;

use serde_json::{Map, Value, json};
use yi_acp::update::{IdMap, base64, extension, replay_updates, to_updates, update_notification};
use yi_acp::{VERSION_MISMATCH_ERROR, negotiate};
use yi_types::acp::{
    AcpNotification, AcpSessionUpdate, AcpState, AcpStopReason, AcpToolCallStatus, AcpUpdateParams,
};
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
        assistant_message_event: AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: text.to_owned(),
        },
    };
    let first = to_updates(&delta("hel"), &mut ids); // codespell:ignore hel
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
    assert_eq!(json["content"]["text"], "hel"); // codespell:ignore hel
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

/// The console notebook draws a cell's image from the update's content, since the
/// details record no longer carries the bytes.
#[test]
fn a_tool_result_image_reaches_the_tool_call_content() -> TestResult {
    let end = to_updates(
        &AgentEvent::ToolExecutionEnd {
            tool_call_id: "t1".to_owned(),
            tool_name: "ipython".to_owned(),
            result: ToolResult {
                content: vec![
                    Content::Text {
                        text: "attached".to_owned(),
                        text_signature: None,
                    },
                    Content::Image {
                        data: "iVBORw0KGgo=".to_owned(),
                        mime_type: "image/png".to_owned(),
                    },
                ],
                details: json!({}),
                usage: None,
                added_tool_names: None,
                terminate: None,
            },
            is_error: false,
        },
        &mut IdMap::new(1000),
    );
    let json = serde_json::to_value(&end)?;
    assert_eq!(
        json[0]["content"],
        json!([
            {"type": "content", "content": {"type": "text", "text": "attached"}},
            {"type": "content", "content": {"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"}},
        ])
    );
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
fn child_updates_become_a_subagent_update_notification() -> TestResult {
    let mut ids = IdMap::new(1000);
    let updates = to_updates(
        &AgentEvent::ChildUpdate {
            update: yi_types::subagent::ChildUpdate {
                id: yi_types::subagent::ChildId("sub-abc".to_owned()),
                name: "sweeper".to_owned(),
                status: yi_types::subagent::ChildStatus::Running,
                activity: yi_types::subagent::ChildActivity::Executing,
                tool_use_count: 3,
                token_count: 1200,
                answer_preview: None,
                error: None,
                exit: None,
                flag: None,
            },
        },
        &mut ids,
    );
    let json = serde_json::to_value(&updates[0])?;
    assert_eq!(json["sessionUpdate"], "_yi/subagent_update");
    assert_eq!(json["id"], "sub-abc");
    assert_eq!(json["activity"], "executing");
    assert_eq!(json["toolUseCount"], 3);
    assert_eq!(json["tokenCount"], 1200);
    assert!(
        json.get("answerPreview").is_none(),
        "an absent preview stays absent on the wire: {json}"
    );
    Ok(())
}

/// The hand-built frame is the wire the serde types spell, for extensions and standard kinds.
#[test]
fn update_notifications_match_the_serde_wire_shape() -> TestResult {
    let updates = [
        extension(
            "_yi/replay",
            [("entries", json!([{"a": 1}])), ("from", json!(0))],
        ),
        AcpSessionUpdate::StateUpdate(AcpState::Running),
    ];
    for update in updates {
        let expected = serde_json::to_value(AcpNotification {
            jsonrpc: "2.0".to_owned(),
            method: "session/update".to_owned(),
            params: serde_json::to_value(AcpUpdateParams {
                session_id: "s1".to_owned(),
                update: update.clone(),
            })?,
        })?;
        assert_eq!(update_notification("s1", update), expected);
    }
    Ok(())
}

#[test]
fn base64_pads_correctly() {
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8="); // codespell:ignore fo
    assert_eq!(base64(b"foo"), "Zm9v");
    assert_eq!(base64(b"foob"), "Zm9vYg==");
}

#[test]
fn replay_walks_entries_into_full_message_updates() -> TestResult {
    let entries = vec![
        Entry::Message {
            id: "e1".to_owned(),
            message: AgentMessage::user_input(UserContent::Text("fix the bug".to_owned()), 0),
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

/// Dies with host notices sent as `user_message`: a stock client drew "[subagent writer
/// finished]" as if the user had typed it, live and on replay. Dies too with the notice as a
/// custom update no stock client decodes, and with a prompt typed before D25 replayed as a notice.
#[test]
fn a_host_notice_is_never_a_user_message() -> TestResult {
    let notice = AgentMessage::host_user(
        UserContent::Text("[subagent writer finished]".to_owned()),
        0,
    );
    let live = to_updates(
        &AgentEvent::MessageStart {
            message: notice.clone(),
        },
        &mut IdMap::new(1000),
    );
    let entry = Entry::Message {
        id: "e1".to_owned(),
        message: notice,
        terminate: None,
        parent_id: None,
        seq: 1,
        timestamp: yi_types::message::ATTRIBUTED_SINCE_MS,
    };
    let replayed = replay_updates(&[entry], &mut IdMap::new(1000));
    for update in live.iter().chain(&replayed) {
        let json = serde_json::to_value(update)?;
        assert_eq!(json["sessionUpdate"], "agent_message", "{json}");
        let block = &json["content"][0];
        assert_eq!(block["type"], "text", "{json}");
        assert_eq!(block["text"], "[subagent writer finished]", "{json}");
        assert_eq!(block["_meta"]["yi"]["hostNotice"], true, "{json}");
    }
    assert_eq!((live.len(), replayed.len()), (1, 1));
    // A line from a session written on 2026-08-30, before any message carried an attribution.
    let typed = r#"{"kind":"entry","lane":"main","type":"message","id":"01a03699-7a72-77dd-893e-6a5a2644e7e9","message":{"role":"user","content":"hello","timestamp":0},"parentId":null,"seq":1,"timestamp":1787622423154}"#;
    let typed: Entry = serde_json::from_str(typed)?;
    let replayed = serde_json::to_value(replay_updates(&[typed], &mut IdMap::new(1000)))?;
    assert_eq!(replayed[0]["sessionUpdate"], "user_message", "{replayed}");
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
                "result": {"outcome": {"outcome": "selected", "optionId": "allow_always_1"}},
            }));
        }
    });
    let asker = yi_acp::bridge_asker("s1".to_owned(), sink, Arc::clone(&pending));
    let changes = [std::path::PathBuf::from("/repo/src/lib.rs")];
    let grant = |label: &str| yi_runtime::Grant {
        kind: yi_types::permission::RuleKind::FileMutation,
        canonical: label.to_owned(),
        label: label.to_owned(),
    };
    let grants = [
        grant("edits under src"),
        grant("edits anywhere in this tree"),
    ];
    let outcome = asker(&yi_runtime::PermissionAsk {
        title: "write requires permission",
        description: "overwrite /repo/src/lib.rs",
        patch: Some("--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n"),
        changes: &changes,
        grants: &grants,
    });
    assert!(
        matches!(outcome, yi_runtime::AskOutcome::AllowAlways(1)),
        "allow_always_1 must keep the second grant, got {outcome:?}"
    );
    let log = seen.lock().map_err(|error| error.to_string())?;
    let request = log
        .first()
        .ok_or("the request must be written before blocking")?;
    assert_eq!(request["method"], "session/request_permission");
    assert_eq!(request["params"]["sessionId"], "s1");
    assert_eq!(request["params"]["title"], "write requires permission");
    assert_eq!(
        request["params"]["content"][0]["changes"][0], "/repo/src/lib.rs",
        "C7: the paths the call would touch travel with the request"
    );
    assert!(
        request["params"]["content"][0]["patch"]
            .as_str()
            .is_some_and(|patch| patch.contains("+new")),
        "C7: the T13 patch is structured content, not prose in the description: {request}"
    );
    assert_eq!(request["params"]["options"][0]["kind"], "allow_once");
    let names: Vec<&str> = request["params"]["options"]
        .as_array()
        .ok_or("options")?
        .iter()
        .filter_map(|option| option["name"].as_str())
        .collect();
    assert_eq!(
        names,
        [
            "Allow once",
            "Always allow edits under src",
            "Always allow edits anywhere in this tree",
            "Reject"
        ]
    );
    assert!(
        pending
            .lock()
            .map_err(|error| error.to_string())?
            .is_empty(),
        "the pending entry must be cleaned up after the answer"
    );
    Ok(())
}

fn tool_result() -> yi_types::event::ToolResult {
    yi_types::event::ToolResult {
        content: vec![Content::Text {
            text: "ok".to_owned(),
            text_signature: None,
        }],
        details: serde_json::json!({"patch": "+x"}),
        usage: None,
        added_tool_names: None,
        terminate: None,
    }
}

fn child_update() -> yi_types::subagent::ChildUpdate {
    yi_types::subagent::ChildUpdate {
        id: yi_types::subagent::ChildId("sub-abc".to_owned()),
        name: "sweeper".to_owned(),
        status: yi_types::subagent::ChildStatus::Running,
        activity: yi_types::subagent::ChildActivity::Executing,
        tool_use_count: 3,
        token_count: 1200,
        answer_preview: None,
        error: None,
        exit: None,
        flag: None,
    }
}

/// One sample per `AgentEvent` variant, so a variant added later fails here first.
fn every_event() -> Vec<AgentEvent> {
    let user = AgentMessage::host_user(UserContent::Text("sanity".to_owned()), 0);
    vec![
        AgentEvent::AgentStart,
        AgentEvent::AgentEnd {
            messages: vec![assistant_partial()],
        },
        AgentEvent::TurnStart,
        AgentEvent::TurnEnd {
            message: assistant_partial(),
            tool_results: vec![user.clone()],
        },
        AgentEvent::MessageStart {
            message: user.clone(),
        },
        AgentEvent::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "faux: ".to_owned(),
            },
        },
        AgentEvent::MessageEnd {
            message: assistant_partial(),
        },
        AgentEvent::ToolExecutionStart {
            tool_call_id: "call_1".to_owned(),
            tool_name: "bash".to_owned(),
            args: serde_json::json!({"cmd": "ls"}),
        },
        AgentEvent::ToolExecutionUpdate {
            tool_call_id: "call_1".to_owned(),
            tool_name: "bash".to_owned(),
            args: serde_json::json!({"cmd": "ls"}),
            partial_result: tool_result(),
        },
        AgentEvent::ToolExecutionEnd {
            tool_call_id: "call_1".to_owned(),
            tool_name: "bash".to_owned(),
            result: tool_result(),
            is_error: false,
        },
        AgentEvent::PermissionRequested {
            tool_call_id: "call_1".to_owned(),
            title: "run ls".to_owned(),
            description: "lists the tree".to_owned(),
        },
        AgentEvent::PermissionResolved {
            tool_call_id: "call_1".to_owned(),
            allowed: true,
        },
        AgentEvent::ChildUpdate {
            update: child_update(),
        },
    ]
}

/// `_yi/event` carries the runtime event verbatim: what the console decodes is what
/// solo's reducer would have received in-process, for every variant.
#[test]
fn yi_event_round_trips_every_variant() -> TestResult {
    let child = yi_types::subagent::ChildId("sub-1".to_owned());
    for (n, event) in every_event().into_iter().enumerate() {
        let seq = u64::try_from(n)?;
        let update = yi_acp::update::event_update(&event, seq, Some(&child));
        let wire = serde_json::to_value(yi_types::acp::AcpUpdateParams {
            session_id: "s".to_owned(),
            update,
        })?;
        let back: yi_types::acp::AcpUpdateParams = serde_json::from_value(wire)?;
        let AcpSessionUpdate::Extension(extension) = back.update else {
            return Err("an event update must decode as an extension".into());
        };
        assert_eq!(extension.session_update, "_yi/event");
        assert_eq!(extension.fields["seq"], seq);
        assert_eq!(extension.fields["childId"], "sub-1");
        let decoded: AgentEvent = serde_json::from_value(extension.fields["event"].clone())?;
        assert_eq!(decoded, event, "variant {n} changed across the wire");
    }
    let parent = serde_json::to_value(yi_acp::update::event_update(
        &AgentEvent::AgentStart,
        0,
        None,
    ))?;
    assert!(
        parent.get("childId").is_none(),
        "the parent stream carries no childId key: {parent}"
    );
    Ok(())
}
