//! D165: a member's state is read from its own records, and the environment line groups them.
//!
//! F0d, the stuck notice. Plan section 7.5: the probe loop's second job reads
//! `state_from_records` for every running child and sends one `failure`-kind notice
//! `[child <name> stuck: <note>]` through the same wake, latched until that child's records
//! move. Deterministic, from records the loop already writes, with no model in the decision.
//! The row below uses this file's own helpers, `store`, `custom` and `recent_entries`, with
//! `STUCK_IDLE_MS` and the injected `now` that `state_from_records` already takes, and adds
//! the `StuckLatch` keyed by child name.
//!
//! | test | tier | helpers | what it pins | the control it dies with |
//! |---|---|---|---|---|
//! | `a_stuck_child_is_reported_once_until_its_records_move` | T0, clock injected | `store`, `custom`, `recent_entries`, `state_from_records`, `STUCK_IDLE_MS`, `StuckLatch` | A child that goes `Stuck` notices once. Ticking again at a later clock with the same records notices nothing. One new record, and the next tick past `STUCK_IDLE_MS` notices once more. Two children going stuck on the same tick get one notice each, and a child that was already stuck when the latch was built is not announced twice. | The latch being keyed on the child and cleared by its records moving, not by a timer. Key it on the tick and a stuck child reports every 60 seconds forever; clear it on any tick and the same episode is announced again on the next one, which is the notice spam that makes a real one invisible. |

use std::error::Error;
use std::sync::{Arc, Mutex};

use serde_json::{Map, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_runtime::family::{
    MemberState, MemberView, Phase, STUCK_IDLE_MS, StuckLatch, children_line, compact_entry,
    recent_entries, state_from_records,
};
use yi_types::message::{AgentMessage, StopReason, UserContent};
use yi_types::subagent::{ChildExit, FailClass};

const LIVE: (Option<ChildExit>, Phase) = (None, Phase::Live);
const DONE: (Option<ChildExit>, Phase) = (Some(ChildExit::Completed), Phase::Live);

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
fn an_ended_child_reads_finished_or_names_its_error() -> TestResult {
    assert_eq!(
        state_from_records(DONE, None, &[], 1_000_000).0,
        MemberState::Finished
    );
    assert_eq!(
        state_from_records(
            (
                Some(ChildExit::Failed {
                    class: FailClass::Provider
                }),
                Phase::Live
            ),
            Some("boom"),
            &[],
            0
        )
        .1
        .as_deref(),
        Some("boom")
    );
    Ok(())
}

#[test]
fn a_running_child_is_stuck_on_a_loop_record_or_five_idle_minutes() -> TestResult {
    let session = store("stuck");
    yi_session::lock_session(&session).append_message(
        "main",
        custom("repeat_break", json!({"signal": "repeat_break"})),
    )?;
    let recent = recent_entries(&session);
    let now = yi_session::now_ms();
    let (state, note, _) = state_from_records(LIVE, None, &recent, now);
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
    let (state, _, _) = state_from_records(LIVE, None, &recent, stamped + 1_000);
    assert_eq!(state, MemberState::Running);
    let (state, note, idle) = state_from_records(LIVE, None, &recent, stamped + STUCK_IDLE_MS);
    assert_eq!(state, MemberState::Stuck);
    assert_eq!(note.as_deref(), Some("idle 300s"));
    assert_eq!(idle, 300);
    Ok(())
}

/// Dies with the typed read in `signal_of`: match on the record's name again and a record
/// that only looks like the loop's is stuck, while the loop's own under a new name is not.
#[test]
fn stuck_reads_the_typed_signal() -> TestResult {
    let now = yi_session::now_ms();
    for (name, details, stuck) in [
        ("repeat_break", json!({}), None),
        (
            "renamed",
            json!({"signal": "repeat_break"}),
            Some("repeat_break"),
        ),
        (
            "length_redrive",
            json!({"rung": 1, "signal": "length_redrive"}),
            None,
        ),
        (
            "length_redrive",
            json!({"rung": 2, "signal": "length_redrive"}),
            Some("length_redrive rung 2"),
        ),
        (
            "note",
            json!({"signal": "let_go"}),
            Some("todo_intercept let go"),
        ),
        ("note", json!({"signal": "napping"}), None),
    ] {
        let session = store("typed");
        yi_session::lock_session(&session).append_message("main", custom(name, details.clone()))?;
        let (state, note, _) = state_from_records(LIVE, None, &recent_entries(&session), now);
        assert_eq!(
            (state == MemberState::Stuck, note.as_deref()),
            (stuck.is_some(), stuck),
            "{name} {details}"
        );
    }
    let (state, _, _) = state_from_records((None, Phase::Queued), None, &[], now);
    assert_eq!(state, MemberState::Queued, "admitted, not yet polled");
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

fn newest_ms(session: &yi_session::SharedSession) -> u64 {
    recent_entries(session)
        .iter()
        .map(|entry| match entry {
            yi_types::entry::Entry::Message { timestamp, .. } => *timestamp,
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}

/// A running member as its records show it at the injected clock.
fn member(name: &str, session: &yi_session::SharedSession, now_ms: u64) -> MemberView {
    let recent = recent_entries(session);
    let (state, note, idle_s) = state_from_records(LIVE, None, &recent, now_ms);
    MemberView {
        name: name.to_owned(),
        state,
        note,
        tools: 0,
        tokens: 0,
        idle_s,
        worktree: None,
    }
}

#[test]
fn a_stuck_child_is_reported_once_until_its_records_move() -> TestResult {
    let a = store("a");
    let b = store("b");
    for session in [&a, &b] {
        yi_session::lock_session(session).append_message(
            "main",
            faux_assistant_message(vec![faux_text("working")], StopReason::ToolUse),
        )?;
    }
    let stamped = newest_ms(&a).max(newest_ms(&b));
    let mut latch = StuckLatch::default();
    assert!(
        latch
            .notices(&[
                member("a", &a, stamped + 1_000),
                member("b", &b, stamped + 1_000)
            ])
            .is_empty(),
        "a running child earns no notice"
    );
    let idle = stamped + STUCK_IDLE_MS;
    assert_eq!(
        latch.notices(&[member("a", &a, idle), member("b", &b, idle)]),
        vec!["[child a stuck: idle 300s]", "[child b stuck: idle 300s]"],
        "two children going stuck on one tick get one notice each"
    );
    let later = stamped + 2 * STUCK_IDLE_MS;
    assert!(
        latch
            .notices(&[member("a", &a, later), member("b", &b, later)])
            .is_empty(),
        "a later clock over the same records is the same episode"
    );

    yi_session::lock_session(&a).append_message(
        "main",
        faux_assistant_message(vec![faux_text("moving again")], StopReason::ToolUse),
    )?;
    let moved = newest_ms(&a);
    // b's clock stays where it was, so b is still in its first episode.
    assert!(
        latch
            .notices(&[member("a", &a, moved + 1_000), member("b", &b, later)])
            .is_empty(),
        "a record that moves releases the latch without a notice"
    );
    let again = moved + STUCK_IDLE_MS;
    assert_eq!(
        latch.notices(&[member("a", &a, again), member("b", &b, again)]),
        vec!["[child a stuck: idle 300s]"],
        "a new episode is announced once; the child still in its old one is not"
    );

    let mut fresh = StuckLatch::default();
    assert_eq!(
        fresh.notices(&[member("b", &b, again)]).len(),
        1,
        "a child already stuck when the latch is built is announced once"
    );
    assert!(fresh.notices(&[member("b", &b, again)]).is_empty());
    assert!(
        fresh.notices(&[]).is_empty() && fresh.notices(&[member("b", &b, again)]).len() == 1,
        "a reaped child leaves the latch"
    );
    Ok(())
}

/// Dies with the todo branch reading a key the record never carries: a child whose newest todo
/// record blocks an item on the user reads `running`, and its parent never sees the options.
#[test]
fn a_child_that_blocks_a_todo_on_you_needs_you_and_names_its_options() -> TestResult {
    let session = store("asking");
    let record: serde_json::Value = serde_json::from_str(include_str!(
        "../../types/tests/fixtures/todo-record-ask-v1.json"
    ))?;
    yi_session::lock_session(&session).append_custom("main", "todo", Some(record))?;
    let recent = recent_entries(&session);
    let (state, note, _) = state_from_records(LIVE, None, &recent, yi_session::now_ms());
    assert_eq!(
        (state, note.as_deref()),
        (
            MemberState::NeedsYou,
            Some("hero style (blocked on user: which hero?) — 1. Calm · 2. Bold · 3. Dense")
        )
    );
    Ok(())
}

/// Dies with a stale question: the newest todo record answered it, yet an older one still in the
/// recent window makes the child read `needs_you`.
#[test]
fn only_the_newest_todo_record_says_a_child_needs_you() -> TestResult {
    let session = store("answered");
    let mut record: serde_json::Value = serde_json::from_str(include_str!(
        "../../types/tests/fixtures/todo-record-ask-v1.json"
    ))?;
    yi_session::lock_session(&session).append_custom("main", "todo", Some(record.clone()))?;
    yi_session::lock_session(&session).append_message("main", custom("mail", json!({})))?;
    let item = record
        .pointer_mut("/list/phases/0/items/0")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or("fixture item")?;
    item.insert("state".to_owned(), json!("pending"));
    item.remove("on");
    item.remove("note");
    yi_session::lock_session(&session).append_custom("main", "todo", Some(record))?;
    let recent = recent_entries(&session);
    let (state, note, _) = state_from_records(LIVE, None, &recent, yi_session::now_ms());
    assert_eq!((state, note), (MemberState::Running, None));
    Ok(())
}
