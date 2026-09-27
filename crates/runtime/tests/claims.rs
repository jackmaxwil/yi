//! A done todo's evidence held against what the session ledger recorded (D255: display only).
use std::error::Error;

use serde_json::json;
use yi_runtime::todo::claims::claims;
use yi_types::entry::Entry;
use yi_types::todo::{Claim, TodoList};

fn entry(seq: u64, message: serde_json::Value) -> Result<Entry, Box<dyn Error>> {
    Ok(Entry::Message {
        id: format!("e{seq}"),
        message: serde_json::from_value(message)?,
        terminate: None,
        parent_id: None,
        seq,
        timestamp: seq,
    })
}

/// Dies with evidence taken on its word: the circular "t2 closed before the report existed"
/// read exactly like a command whose output the ledger holds.
#[test]
fn a_quoted_span_found_in_a_recorded_call_is_observed_and_anything_else_is_claimed()
-> Result<(), Box<dyn Error>> {
    let usage = json!({"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}});
    let entries = vec![
        entry(
            1,
            json!({"role": "assistant", "api": "faux", "provider": "faux", "model": "faux-1",
            "stopReason": "toolUse", "timestamp": 1, "usage": usage,
            "content": [{"type": "toolCall", "id": "c1", "name": "bash",
                "arguments": {"cmd": "cargo test -p yi-tui"}}]}),
        )?,
        entry(
            2,
            json!({"role": "toolResult", "toolCallId": "c1", "toolName": "bash",
            "isError": false, "timestamp": 2,
            "content": [{"type": "text", "text": "test result: ok. 9 passed; 0 failed"}]}),
        )?,
    ];
    let list: TodoList = serde_json::from_value(json!({"phases": [{"name": "Tasks", "items": [
        {"label": "run the suite", "state": "done", "evidence": "`cargo test -p yi-tui` is green"},
        {"label": "check the count", "state": "done", "evidence": "`test result: ok. 9 passed`"},
        {"label": "write the report", "state": "done", "evidence": "the report is written"},
        {"label": "lint", "state": "done", "evidence": "`cargo clippy` clean"},
        {"label": "land it", "state": "running"}
    ]}]}))?;
    let observed = |label: &str, call: Option<&str>| Claim {
        label: label.to_owned(),
        observed: call.map(str::to_owned),
    };
    assert_eq!(
        claims(&list, &entries),
        vec![
            observed("run the suite", Some("c1")),
            observed("check the count", Some("c1")),
            observed("write the report", None),
            observed("lint", None),
        ]
    );
    Ok(())
}
