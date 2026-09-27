//! A session over wall time, folded from its ledger for the console's Tape.
use std::error::Error;

use serde_json::json;
use yi_types::entry::Entry;
use yi_types::message::{ATTRIBUTED_SINCE_MS, AgentMessage, UserContent};
use yi_types::tape::{Mark, MarkKind};

const T: u64 = ATTRIBUTED_SINCE_MS;

fn message(id: &str, at: u64, message: AgentMessage) -> Entry {
    Entry::Message {
        id: id.to_owned(),
        message,
        terminate: None,
        parent_id: None,
        seq: 0,
        timestamp: T + at,
    }
}

fn parsed(value: serde_json::Value) -> Result<AgentMessage, Box<dyn Error>> {
    Ok(serde_json::from_value(value)?)
}

/// Dies with no view of the time: where 22 minutes went had no answer on screen, and the
/// session's checkpoints and failures sat nowhere a user could scrub to.
#[test]
fn a_tape_splits_model_and_tool_time_and_marks_turns_failures_and_checkpoints()
-> Result<(), Box<dyn Error>> {
    let usage = json!({"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}});
    let reply = |content: serde_json::Value| {
        parsed(
            json!({"role": "assistant", "api": "faux", "provider": "faux", "model": "faux-1",
            "stopReason": "stop", "timestamp": 0, "usage": usage, "content": content}),
        )
    };
    let entries = vec![
        message(
            "u1",
            1_000,
            AgentMessage::user_input(UserContent::Text("fix it".to_owned()), 0),
        ),
        message(
            "a1",
            5_000,
            reply(json!([{"type": "toolCall", "id": "c1", "name": "bash",
            "arguments": {"cmd": "false"}}]))?,
        ),
        message(
            "r1",
            6_000,
            parsed(json!({"role": "toolResult", "toolCallId": "c1",
            "toolName": "bash", "isError": true, "timestamp": 0,
            "content": [{"type": "text", "text": "exit 1"}]}))?,
        ),
        Entry::Custom {
            id: "k1".to_owned(),
            custom_type: "checkpoint".to_owned(),
            data: None,
            parent_id: None,
            seq: 0,
            timestamp: T + 6_100,
        },
        message(
            "a2",
            9_000,
            reply(json!([{"type": "text", "text": "done"}]))?,
        ),
    ];
    let tape = yi_runtime::tape::tape(&entries);
    assert_eq!((tape.start, tape.end), (T + 1_000, T + 9_000));
    assert_eq!(
        tape.model,
        vec![[T + 1_000, T + 5_000], [T + 6_100, T + 9_000]]
    );
    assert_eq!(tape.tools, vec![[T + 5_000, T + 6_000]]);
    let kinds: Vec<(MarkKind, &str)> = tape
        .marks
        .iter()
        .map(|Mark { kind, entry, .. }| (*kind, entry.as_str()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (MarkKind::User, "u1"),
            (MarkKind::Failed, "r1"),
            (MarkKind::Checkpoint, "k1")
        ]
    );
    Ok(())
}
