//! A done todo's evidence held against what the session ledger recorded (D255: display only).
use std::error::Error;

use serde_json::json;
use yi_runtime::todo::claims::{History, claims};
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
/// read like a command the ledger holds, and the todo call quoting it observed itself.
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
        entry(
            3,
            json!({"role": "assistant", "api": "faux", "provider": "faux", "model": "faux-1",
            "stopReason": "toolUse", "timestamp": 3, "usage": usage,
            "content": [{"type": "toolCall", "id": "c2", "name": "todo",
                "arguments": {"op": "done", "label": "lint", "evidence": "`cargo clippy` clean"}}]}),
        )?,
        entry(
            4,
            json!({"role": "toolResult", "toolCallId": "c2", "toolName": "todo",
            "isError": false, "timestamp": 4,
            "content": [{"type": "text", "text": "done: lint (`cargo clippy` clean)"}]}),
        )?,
    ];
    let list: TodoList = serde_json::from_value(json!({"phases": [{"name": "Tasks", "items": [
        {"label": "run the suite", "state": "done", "evidence": "`cargo test -p yi-tui` is green"},
        {"label": "check the count", "state": "done", "evidence": "`test result: ok. 9 passed`"},
        {"label": "write the report", "state": "done", "evidence": "the report is written"},
        {"label": "lint", "state": "done", "evidence": "`cargo clippy` clean"},
        {"label": "ship", "state": "done"},
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
            observed("ship", None),
        ]
    );
    Ok(())
}

#[test]
fn incremental_history_scans_only_new_entries_and_keeps_older_evidence()
-> Result<(), Box<dyn Error>> {
    let store = std::sync::Arc::new(std::sync::Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: "incremental-claims".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        },
    )));
    let first = entry(
        2,
        json!({"role": "toolResult", "toolCallId": "c1", "toolName": "bash",
            "isError": false, "timestamp": 2,
            "content": [{"type": "text", "text": "test result: ok. 9 passed"}]}),
    )?;
    yi_session::lock_session(&store).append_entry(first, "main")?;
    let list = |label: &str, evidence: &str| {
        serde_json::from_value(json!({"phases": [{"name": "Tasks", "items": [
            {"label": label, "state": "done", "evidence": evidence}
        ]}]}))
    };
    let mut history = History::default();
    assert_eq!(history.observe(&store), 1, "the first read seeds the cache");
    assert_eq!(
        history.claims_cached(&list("first", "`test result: ok. 9 passed`")?, &store, 0),
        vec![Claim {
            label: "first".to_owned(),
            observed: Some("c1".to_owned())
        }]
    );
    let later = entry(
        3,
        json!({"role": "toolResult", "toolCallId": "c3", "toolName": "bash",
            "isError": false, "timestamp": 3,
            "content": [{"type": "text", "text": "second proof"}]}),
    )?;
    yi_session::lock_session(&store).append_entry(later, "main")?;
    assert_eq!(
        history.observe(&store),
        1,
        "only the appended entry is read"
    );
    let second = serde_json::from_value(json!({"phases": [{"name": "Tasks", "items": [
        {"label": "first", "state": "done", "evidence": "`test result: ok. 9 passed`"},
        {"label": "second", "state": "done", "evidence": "`second proof`"}
    ]}]}))?;
    assert_eq!(
        history.claims_cached(&second, &store, 1),
        vec![
            Claim {
                label: "first".to_owned(),
                observed: Some("c1".to_owned())
            },
            Claim {
                label: "second".to_owned(),
                observed: Some("c3".to_owned())
            },
        ]
    );
    Ok(())
}

#[test]
fn late_evidence_upgrades_a_claim_without_another_todo_write() -> Result<(), Box<dyn Error>> {
    let store = std::sync::Arc::new(std::sync::Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: "late-claims".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        },
    )));
    let list: TodoList = serde_json::from_value(json!({"phases": [{"name": "Tasks", "items": [
        {"label": "prove it", "state": "done", "evidence": "`proof arrived`"}
    ]}]}))?;
    let mut history = History::default();
    assert_eq!(
        history.claims_cached(&list, &store, 7),
        vec![Claim {
            label: "prove it".to_owned(),
            observed: None,
        }]
    );
    yi_session::lock_session(&store).append_entry(
        entry(
            3,
            json!({"role": "toolResult", "toolCallId": "c3", "toolName": "bash",
                "isError": false, "timestamp": 3,
                "content": [{"type": "text", "text": "proof arrived"}]}),
        )?,
        "main",
    )?;
    assert_eq!(
        history.claims_cached(&list, &store, 7),
        vec![Claim {
            label: "prove it".to_owned(),
            observed: Some("c3".to_owned()),
        }]
    );
    Ok(())
}
