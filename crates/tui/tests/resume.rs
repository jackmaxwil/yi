//! A session resumed into a pane reads as it did live: the same cards, the same context figure.
use crate::common;

use std::error::Error;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use serde_json::{Value, json};
use yi_tui::app::{App, TuiOptions};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::port::Reply;
use yi_types::entry::Entry;
use yi_types::event::AgentEvent;
use yi_types::message::{ATTRIBUTED_SINCE_MS, AgentMessage, UserContent};

type TestResult = Result<(), Box<dyn Error>>;

fn app() -> App {
    App::new(
        TuiOptions {
            model: common::test_model("faux-1"),
            session_name: "resume".to_owned(),
            cwd: "/tmp".to_owned(),
            lane: None,
            context_window: 128_000,
            session_dir: String::new(),
            keys: Vec::new(),
            initial_prompt: None,
            pace: 0,
        },
        Theme::new(ColorTier::TrueColor, true),
        yi_tui::keymap::default_keymap(),
        80,
    )
}

fn entry(seq: u64, message: AgentMessage) -> Entry {
    Entry::Message {
        id: format!("e{seq}"),
        message,
        terminate: None,
        parent_id: None,
        seq,
        timestamp: ATTRIBUTED_SINCE_MS + seq,
    }
}

fn typed(text: &str) -> AgentMessage {
    AgentMessage::user_input(UserContent::Text(text.to_owned()), 0)
}

fn reply(content: Value, total: i64, unknown: bool) -> Result<AgentMessage, Box<dyn Error>> {
    Ok(serde_json::from_value(json!({
        "role": "assistant", "content": content, "api": "faux", "provider": "faux",
        "model": "faux-1", "stopReason": "stop", "timestamp": 0,
        "usage": {"input": total, "output": 0, "cacheRead": 0, "cacheWrite": 0,
            "totalTokens": total, "unknown": unknown,
            "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}},
    }))?)
}

fn result(id: &str, tool: &str, text: &str) -> Result<AgentMessage, Box<dyn Error>> {
    Ok(serde_json::from_value(json!({
        "role": "toolResult", "toolCallId": id, "toolName": tool, "isError": false,
        "content": [{"type": "text", "text": text}], "timestamp": 0,
    }))?)
}

fn flat(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

fn status_row(app: &mut App) -> Result<String, Box<dyn Error>> {
    let area = Rect::new(0, 0, 80, 12);
    let mut buffer = Buffer::empty(area);
    let _ = yi_tui::render::paint_pane(app, None, &mut buffer, area, &mut 0);
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_owned()))
                .collect::<String>()
        })
        .find(|row| row.contains("/ 128K"))
        .ok_or_else(|| "no context figure on the status row".into())
}

/// Dies with the figure copied from the last reply: a reply with no usage object read as 0, and
/// a resumed pane showed 0 until its next reply ended.
#[test]
fn a_resumed_pane_shows_the_context_its_last_counted_reply_reported() -> TestResult {
    let mut app = app();
    app.apply(Reply::History(vec![
        entry(1, typed("look at the tools")),
        entry(
            2,
            reply(json!([{"type": "text", "text": "reading"}]), 103_212, false)?,
        ),
        entry(
            3,
            reply(json!([{"type": "text", "text": "no usage"}]), 0, true)?,
        ),
    ]));
    let row = status_row(&mut app)?;
    assert!(row.contains("103,212 / 128K"), "resumed: {row:?}");
    app.reduce_agent(AgentEvent::MessageEnd {
        message: reply(json!([{"type": "text", "text": "again"}]), 0, true)?,
    });
    let row = status_row(&mut app)?;
    assert!(
        row.contains("103,212 / 128K"),
        "after a usage-less reply: {row:?}"
    );
    Ok(())
}

/// Dies with the old replay: `→ Read` with no path, a card per todo step, and no thought or
/// divider, where the live turn had drawn all of them.
#[test]
fn a_replayed_turn_reads_as_the_live_one_did() -> TestResult {
    let calls = json!([
        {"type": "thinking", "thinking": "the manifest names the crate"},
        {"type": "toolCall", "id": "c1", "name": "read", "arguments": {"path": "Cargo.toml"}},
        {"type": "toolCall", "id": "c2", "name": "todo", "arguments": {"op": "start", "id": "t1"}},
    ]);
    let mut app = app();
    app.replay_entries(&[
        entry(1, typed("first question")),
        entry(2, reply(calls, 900, false)?),
        entry(3, result("c1", "read", "1:[package]\n2:name = \"yi\"")?),
        entry(4, result("c2", "todo", "Todos 0/2 · running: t1")?),
        entry(5, typed("second question")),
    ]);
    let lines = flat(&app.take_commits());
    let at = |needle: &str| lines.iter().position(|line| line.contains(needle));
    assert!(
        at("Cargo.toml").is_some(),
        "the read names its file: {lines:#?}"
    );
    assert!(
        at("the manifest names the crate").is_some(),
        "the thought replays: {lines:#?}"
    );
    assert!(
        at("Todo").is_none(),
        "a todo step is the HUD's, not a card: {lines:#?}"
    );
    let (first, second) = (at("first question"), at("second question"));
    let divided = lines.iter().enumerate().any(|(i, line)| {
        line.trim_start().starts_with("──") && Some(i) > first && Some(i) < second
    });
    assert!(divided, "a divider opens the second turn: {lines:#?}");
    Ok(())
}

fn priced(text: &str, cost: f64, unknown: bool) -> Result<AgentMessage, Box<dyn Error>> {
    Ok(serde_json::from_value(json!({
        "role": "assistant", "content": [{"type": "text", "text": text}], "api": "faux",
        "provider": "faux", "model": "faux-1", "stopReason": "stop", "timestamp": 0,
        "usage": {"input": 400, "output": 20, "cacheRead": 9_600, "cacheWrite": 0,
            "totalTokens": 10_020, "unknown": unknown,
            "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": cost}},
    }))?)
}

/// Dies with a restarted `yi` showing $0 for a session that had spent: the row's dollars were a
/// sum of live replies. A second load (a rewind, a reconnect) must set the figure, not add to it.
#[test]
fn a_resumed_session_shows_its_stored_spend_and_cache_rate_once() -> TestResult {
    use std::sync::Arc;
    use yi_runtime::session_store::{CreateOptions, JsonlRepo, SessionRepo, lock_session};
    let dir = crate::scratch::Scratch::new("yi-tui-resume-spend")?;
    let mut repo = JsonlRepo::new(dir.to_path_buf(), "/tmp".to_owned());
    let store = repo.create(CreateOptions::default())?;
    {
        let mut ledger = lock_session(&store);
        ledger.append_message("main", typed("price the build"))?;
        ledger.append_message("main", priced("costed", 0.1, false)?)?;
        ledger.append_message("main", priced("unreported", 0.0, true)?)?;
        let id = ledger.next_id();
        ledger.append_record(serde_json::from_value(json!({"type": "usage", "id": id,
            "lane": "main", "cause": "child_usage_attributed", "seq": 0, "timestamp": 0,
            "usage": {"input": 100, "output": 10, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 110, "cost": {"input": 0, "output": 0, "cacheRead": 0,
                "cacheWrite": 0, "total": 0.05}}}))?)?;
    }
    let session = Arc::new(yi_runtime::AgentSession::new(
        yi_runtime::SessionConfig {
            system_prompt: String::new(),
            model: common::test_model("faux-1"),
            thinking_level: None,
            tool_execution: yi_runtime::ExecutionMode::Sequential,
        },
        Arc::new(yi_runtime::ProviderStream::new(None)),
    ));
    session.attach_store(store)?;
    let mut port = Arc::clone(&session);
    let mut app = app();
    for load in ["first", "second"] {
        app.load_history(&mut port);
        let row = status_row(&mut app)?;
        assert!(
            row.contains("≥$0.150") && row.contains("95% cached"),
            "{load} load: {row:?}"
        );
    }
    Ok(())
}
