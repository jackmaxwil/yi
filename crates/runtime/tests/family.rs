//! D165: a member's state is read from its own records, and the environment line groups them.

use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::{Map, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_runtime::family::{
    MemberState, MemberView, STUCK_IDLE_MS, children_line, compact_entry, recent_entries,
    state_from_records,
};
use yi_types::message::{AgentMessage, StopReason, UserContent};
use yi_types::subagent::ChildStatus;

type TestResult = Result<(), Box<dyn Error>>;

fn store(name: &str) -> yi_session::SharedSession {
    Arc::new(Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: name.to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        },
    )))
}

fn custom(custom_type: &str, details: serde_json::Value) -> AgentMessage {
    AgentMessage::Custom {
        custom_type: custom_type.to_owned(),
        content: UserContent::Text("hidden".to_owned()),
        display: false,
        details: Some(details),
        timestamp: 0,
    }
}

#[test]
fn a_child_that_ended_on_ask_user_needs_you() -> TestResult {
    let mut args = Map::new();
    args.insert(
        "question".to_owned(),
        json!("Which port does the proxy use?"),
    );
    let messages = vec![faux_assistant_message(
        vec![faux_tool_call("q1", "ask_user", args)],
        StopReason::ToolUse,
    )];
    let (state, note, _) =
        state_from_records(ChildStatus::Completed, None, &messages, &[], 1_000_000);
    assert_eq!(state, MemberState::NeedsYou);
    assert_eq!(note.as_deref(), Some("Which port does the proxy use?"));
    let done = vec![faux_assistant_message(
        vec![faux_text("done")],
        StopReason::Stop,
    )];
    assert_eq!(
        state_from_records(ChildStatus::Completed, None, &done, &[], 1_000_000).0,
        MemberState::Finished
    );
    assert_eq!(
        state_from_records(ChildStatus::Error, Some("boom"), &[], &[], 0)
            .1
            .as_deref(),
        Some("boom")
    );
    Ok(())
}

#[test]
fn a_running_child_is_stuck_on_a_loop_record_or_five_idle_minutes() -> TestResult {
    let session = store("stuck");
    yi_session::lock_session(&session).append_message("main", custom("repeat_break", json!({})))?;
    let recent = recent_entries(&session);
    let now = yi_session::now_ms();
    let (state, note, _) = state_from_records(ChildStatus::Running, None, &[], &recent, now);
    assert_eq!(
        (state, note.as_deref()),
        (MemberState::Stuck, Some("repeat_break"))
    );

    let quiet = store("quiet");
    yi_session::lock_session(&quiet).append_message(
        "main",
        faux_assistant_message(vec![faux_text("working")], StopReason::ToolUse),
    )?;
    let recent = recent_entries(&quiet);
    let stamped = recent
        .first()
        .map(|entry| match entry {
            yi_types::entry::Entry::Message { timestamp, .. } => *timestamp,
            _ => 0,
        })
        .unwrap_or(0);
    let (state, _, _) =
        state_from_records(ChildStatus::Running, None, &[], &recent, stamped + 1_000);
    assert_eq!(state, MemberState::Running);
    let (state, note, idle) = state_from_records(
        ChildStatus::Running,
        None,
        &[],
        &recent,
        stamped + STUCK_IDLE_MS,
    );
    assert_eq!(state, MemberState::Stuck);
    assert_eq!(note.as_deref(), Some("idle 300s"));
    assert_eq!(idle, 300);
    Ok(())
}

#[test]
fn the_children_line_groups_members_by_state_with_their_notes() {
    let view = |name: &str, state: MemberState, note: Option<&str>| MemberView {
        name: name.to_owned(),
        state,
        note: note.map(str::to_owned),
        tools: 0,
        tokens: 0,
        idle_s: 0,
        worktree: None,
    };
    assert_eq!(children_line(&[]), None);
    let line = children_line(&[
        view("a", MemberState::Running, None),
        view("b", MemberState::Running, None),
        view("c", MemberState::Finished, None),
        view("d", MemberState::NeedsYou, Some("asked a question")),
        view("e", MemberState::Stuck, Some("repeat_break")),
    ]);
    assert_eq!(
        line.as_deref(),
        Some(
            "children: 2 running (a, b) · 1 finished (c) · 1 needs you (d: asked a question) · 1 stuck (e: repeat_break)"
        )
    );
}

#[test]
fn a_compact_entry_is_one_line_with_its_sequence() -> TestResult {
    let session = store("compact");
    let mut args = Map::new();
    args.insert("command".to_owned(), json!("pytest -q"));
    yi_session::lock_session(&session).append_message(
        "main",
        faux_assistant_message(
            vec![
                faux_text("running the check"),
                faux_tool_call("b1", "bash", args),
            ],
            StopReason::ToolUse,
        ),
    )?;
    let entries = recent_entries(&session);
    let line = compact_entry(entries.first().ok_or("entry")?);
    assert!(
        line.starts_with("#1 assistant: running the check bash "),
        "{line}"
    );
    assert!(line.contains("pytest -q"), "{line}");
    Ok(())
}
