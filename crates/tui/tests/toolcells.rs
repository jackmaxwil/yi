use crate::common;

use std::error::Error;

use ratatui::text::Line;
use serde_json::json;
use yi_tui::app::{App, TuiOptions};
use yi_tui::cell::{Cell, ToolCell, ToolStatus, TranscriptMode};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::keymap::default_keymap;
use yi_types::event::{AgentEvent, ToolResult};
use yi_types::message::Content;

type TestResult = Result<(), Box<dyn Error>>;

fn theme() -> Theme {
    Theme::new(ColorTier::TrueColor, true)
}

fn app() -> App {
    App::new(
        TuiOptions {
            model: common::test_model("faux-1"),
            session_name: "cells".to_owned(),
            cwd: "/tmp".to_owned(),
            lane: None,
            context_window: 128_000,
            session_dir: String::new(),
            keys: Vec::new(),
            initial_prompt: None,
            pace: 0,
        },
        theme(),
        default_keymap(),
        80,
    )
}

fn flat(lines: &[Line<'static>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn started(app: &mut App, id: &str, tool: &str, arg: &str) {
    app.reduce_agent(AgentEvent::ToolExecutionStart {
        tool_call_id: id.to_owned(),
        tool_name: tool.to_owned(),
        args: json!({ "path": arg, "pattern": arg, "cmd": arg }),
    });
}

fn ended(app: &mut App, id: &str, tool: &str, text: &str) {
    ended_with(app, id, tool, text, false, serde_json::Value::Null);
}

fn ended_with(
    app: &mut App,
    id: &str,
    tool: &str,
    text: &str,
    is_error: bool,
    details: serde_json::Value,
) {
    app.reduce_agent(AgentEvent::ToolExecutionEnd {
        tool_call_id: id.to_owned(),
        tool_name: tool.to_owned(),
        result: ToolResult {
            content: vec![Content::Text {
                text: text.to_owned(),
                text_signature: None,
            }],
            details,
            usage: None,
            added_tool_names: None,
            terminate: None,
        },
        is_error,
    });
}

/// Eight rows of looking crowds out the answer that follows it, so the run
/// groups under one bullet with a verb column.
#[test]
fn a_run_of_read_only_calls_commits_as_one_cell() -> TestResult {
    let mut app = app();
    for (index, (tool, arg)) in [
        ("read", "src/lib.rs"),
        ("grep", "ToolCell"),
        ("glob", "**/*.rs"),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("t{index}");
        started(&mut app, &id, tool, arg);
        ended(&mut app, &id, tool, "1:one\n2:two");
    }
    // Nothing is committed while the run is still open.
    assert!(app.take_commits().is_empty());

    app.reduce_agent(AgentEvent::AgentEnd {
        messages: Vec::new(),
    });
    let committed = flat(&app.take_commits());
    let joined = committed.join("\n");
    assert!(joined.contains("✱ Explored ×3"), "{joined}");
    assert!(joined.contains("Read   src/lib.rs"), "{joined}");
    assert!(joined.contains("Search"), "{joined}");
    assert!(joined.contains("List"), "{joined}");
    // One bullet, not three.
    assert_eq!(committed.iter().filter(|row| row.contains('✱')).count(), 1);
    Ok(())
}

/// A single call must not gain a group header it does not need.
#[test]
fn a_lone_read_still_renders_as_itself() -> TestResult {
    let mut app = app();
    started(&mut app, "t0", "read", "src/lib.rs");
    ended(&mut app, "t0", "read", "1:one");
    app.reduce_agent(AgentEvent::AgentEnd {
        messages: Vec::new(),
    });
    let joined = flat(&app.take_commits()).join("\n");
    assert!(!joined.contains("Explored"), "{joined}");
    assert!(joined.contains("→ Read src/lib.rs"), "{joined}");
    Ok(())
}

/// A write is not read-only: it closes the run rather than joining it, and the
/// group must land before the write, in the order they happened.
#[test]
fn a_writing_call_closes_the_run_in_order() -> TestResult {
    let mut app = app();
    for index in 0..2 {
        let id = format!("r{index}");
        started(&mut app, &id, "read", "a.rs");
        ended(&mut app, &id, "read", "1:one");
    }
    started(&mut app, "w", "bash", "cargo test");
    ended(&mut app, "w", "bash", "ok");
    let committed = flat(&app.take_commits());
    let explored = committed.iter().position(|row| row.contains("Explored"));
    let bash = committed
        .iter()
        .position(|row| row.contains("$ cargo test"));
    assert!(explored < bash, "{committed:?}");
    assert!(explored.is_some() && bash.is_some(), "{committed:?}");
    Ok(())
}

/// The user is being asked about one specific call; colouring only the prompt
/// leaves the transcript showing it as an ordinary running row.
#[test]
fn a_call_held_at_the_permission_gate_says_so() -> TestResult {
    let mut app = app();
    started(&mut app, "t0", "bash", "rm -rf build");
    app.reduce_agent(AgentEvent::PermissionRequested {
        tool_call_id: "t0".to_owned(),
        title: "bash".to_owned(),
        description: "rm -rf build".to_owned(),
    });
    assert_eq!(
        app.live_tool_status("t0").ok_or("no live tool")?,
        ToolStatus::Awaiting
    );
    let rendered = flat(
        &Cell::Tool(ToolCell {
            name: String::new(),
            call_id: String::new(),
            intent: None,
            status: ToolStatus::Awaiting,
            summary: ToolCell::summary_of("bash", "rm -rf build"),
            digest: None,
            preview: Vec::new(),
            elapsed_ms: 0,
            calls: 1,
            details: json!({}),
        })
        .lines(80, &theme(), TranscriptMode::Normal, 0),
    );
    assert!(rendered.join("").contains('△'), "{rendered:?}");

    app.reduce_agent(AgentEvent::PermissionResolved {
        tool_call_id: "t0".to_owned(),
        allowed: true,
    });
    assert_eq!(app.live_tool_status("t0"), Some(ToolStatus::Running));
    Ok(())
}

fn edit_cell(path: &str) -> ToolCell {
    ToolCell {
        name: "edit".to_owned(),
        call_id: String::new(),
        intent: None,
        status: ToolStatus::Done,
        summary: ToolCell::summary_of("edit", path),
        digest: Some("updated".to_owned()),
        preview: Vec::new(),
        elapsed_ms: 0,
        calls: 1,
        details: json!({
            "patch": format!("--- a/{path}\n+++ b/{path}\n@@ -1,1 +1,1 @@\n-one\n+two\n"),
            "added": 1,
            "removed": 1,
        }),
    }
}

/// A blank separates blocks and a run of one-line calls packs flush. Both
/// halves matter: always blank is as wrong as never blank.
#[test]
fn spacing_separates_blocks_but_not_one_line_rows() -> TestResult {
    let mut rows = app();
    started(&mut rows, "b0", "bash", "true");
    ended(&mut rows, "b0", "bash", "");
    started(&mut rows, "b1", "bash", "false");
    ended(&mut rows, "b1", "bash", "");
    let packed = flat(&rows.take_commits());
    assert!(
        !packed.iter().any(|row| row.is_empty()),
        "one-line rows pack flush: {packed:?}"
    );

    let mut blocks = app();
    blocks.commit_cell(&Cell::Tool(edit_cell("a.rs")));
    blocks.commit_cell(&Cell::Tool(edit_cell("b.rs")));
    let spaced = flat(&blocks.take_commits());
    assert!(
        spaced.iter().any(|row| row.is_empty()),
        "two multi-line blocks are separated: {spaced:?}"
    );
    Ok(())
}

/// A finished card showed one line of a forty-line result and no sign of the rest.
#[test]
fn a_collapsed_card_shows_three_rows_and_counts_the_rest() -> TestResult {
    let mut app = app();
    let text: Vec<String> = (1..=40).map(|n| format!("line {n}")).collect();
    started(&mut app, "b0", "bash", "seq 40");
    ended_with(
        &mut app,
        "b0",
        "bash",
        &text.join("\n"),
        false,
        json!({ "exitCode": 0 }),
    );
    let rows = flat(&app.take_commits());
    let joined = rows.join("\n");
    for needle in ["line 1", "line 2", "line 3", "… 37 more lines"] {
        assert!(joined.contains(needle), "{needle}: {joined}");
    }
    assert!(!joined.contains("line 4"), "{joined}");
    assert!(
        !joined.contains("exit"),
        "a zero exit says nothing: {joined}"
    );
    assert_eq!(
        rows.iter().filter(|row| row.contains("line 1")).count(),
        1,
        "the first row is not repeated as a digest: {joined}"
    );
    Ok(())
}

/// The old `⏎ 0` chip sat on every card; a pipeline's tail hid a `command not found` under it.
#[test]
fn a_nonzero_exit_and_a_masked_not_found_are_the_chips() -> TestResult {
    let mut app = app();
    started(&mut app, "b0", "bash", "false");
    ended_with(
        &mut app,
        "b0",
        "bash",
        "",
        false,
        json!({ "exitCode": 127 }),
    );
    let joined = flat(&app.take_commits()).join("\n");
    assert!(joined.contains("exit 127"), "{joined}");
    assert!(!joined.contains('⏎'), "{joined}");

    started(&mut app, "b1", "bash", "fgj pr ls | head -20; echo ---");
    ended_with(
        &mut app,
        "b1",
        "bash",
        "sh: fgj: command not found\n---",
        false,
        json!({ "exitCode": 0 }),
    );
    let joined = flat(&app.take_commits()).join("\n");
    assert!(joined.contains("not found"), "{joined}");
    assert!(!joined.contains("exit 0"), "{joined}");
    Ok(())
}

/// Dies with a command that ran and failed drawn as done: the model's flag no longer marks a
/// non-zero exit or a rule's verdict, so the card reads them from `details` instead.
#[test]
fn a_nonzero_exit_and_a_verdict_still_open_with_a_cross() -> TestResult {
    let mut app = app();
    started(&mut app, "b0", "bash", "python3 -m unittest -q");
    ended_with(
        &mut app,
        "b0",
        "bash",
        "FAILED (failures=1)",
        false,
        json!({ "exitCode": 1 }),
    );
    let rows = flat(&app.take_commits());
    assert!(
        rows.iter().any(|row| row.contains("✕ $ python3")),
        "{rows:?}"
    );
    started(&mut app, "p0", "plan", "add_edge");
    ended_with(
        &mut app,
        "p0",
        "plan",
        "plan update refused: ordering cycle through [\"a\", \"b\"]",
        false,
        json!({ "errorKind": "verdict" }),
    );
    let rows = flat(&app.take_commits());
    assert!(rows.iter().any(|row| row.contains('✕')), "{rows:?}");
    Ok(())
}

#[test]
fn a_failed_card_opens_with_a_cross_and_keeps_its_reason() -> TestResult {
    let mut app = app();
    started(&mut app, "t0", "todo", "done t2");
    ended_with(
        &mut app,
        "t0",
        "todo",
        "done needs evidence: the command you ran",
        true,
        json!({}),
    );
    let rows = flat(&app.take_commits());
    assert!(rows.iter().any(|row| row.contains("✕ ⚙ Todo")), "{rows:?}");
    assert!(
        rows.iter().any(|row| row.contains("done needs evidence")),
        "{rows:?}"
    );
    Ok(())
}

/// The head cut at 60 characters while the row had room to 120 and wraps past it.
#[test]
fn a_long_command_keeps_its_head_to_one_hundred_and_twenty() -> TestResult {
    let mut app = app();
    let command = format!("grep -rn {} crates/*/src --include='*.rs'", "x".repeat(70));
    started(&mut app, "b0", "bash", &command);
    ended(&mut app, "b0", "bash", "");
    let joined = flat(&app.take_commits()).join("\n");
    assert!(
        joined.contains("crates/*/src --include='*.rs'"),
        "the head wraps instead of cutting: {joined}"
    );
    assert!(joined.contains(&"x".repeat(70)), "{joined}");
    assert!(!joined.contains('…'), "{joined}");
    Ok(())
}

fn ended_todo(app: &mut App, id: &str, text: &str, is_error: bool) {
    app.reduce_agent(AgentEvent::ToolExecutionEnd {
        tool_call_id: id.to_owned(),
        tool_name: "todo".to_owned(),
        result: ToolResult {
            content: vec![Content::Text {
                text: text.to_owned(),
                text_signature: None,
            }],
            details: serde_json::Value::Null,
            usage: None,
            added_tool_names: None,
            terminate: None,
        },
        is_error,
    });
}

/// Six todo steps in one turn were six cards; the HUD block above the composer carries
/// the list, so a step commits nothing, a refusal keeps its card, and the close is one row.
#[test]
fn todo_steps_live_in_the_hud_not_the_transcript() -> TestResult {
    let mut app = app();
    started(&mut app, "t0", "todo", "start t1");
    ended_todo(
        &mut app,
        "t0",
        "Todos 0/2 · running: read\n- [>] read\n- [ ] write",
        false,
    );
    assert!(
        app.take_commits().is_empty(),
        "a step that succeeded commits no card"
    );
    started(&mut app, "t1", "todo", "done t1");
    ended_todo(
        &mut app,
        "t1",
        "done needs evidence: the command you ran",
        true,
    );
    let refused = flat(&app.take_commits()).join("\n");
    assert!(refused.contains("done needs evidence"), "{refused}");
    started(&mut app, "t2", "todo", "done t2");
    ended_todo(&mut app, "t2", "Todos 2/2\n- [x] read\n- [x] write", false);
    let closed = flat(&app.take_commits()).join("\n");
    assert!(closed.contains("↳ Todos 2/2"), "{closed}");
    assert!(!closed.contains("⚙"), "{closed}");
    Ok(())
}

fn custom(app: &mut App, kind: &str, text: &str, display: bool) {
    app.reduce_agent(AgentEvent::MessageEnd {
        message: yi_types::message::AgentMessage::Custom {
            custom_type: kind.to_owned(),
            content: yi_types::message::UserContent::Text(text.to_owned()),
            display,
            details: None,
            timestamp: 0,
        },
    });
}

/// The todo prelude restated the whole checklist as a dozen flagged rows under the
/// user's prompt; a message sent `display: false` is the model's alone.
#[test]
fn a_hidden_custom_message_stays_out_of_the_transcript() -> TestResult {
    let mut app = app();
    custom(
        &mut app,
        "todo_prelude",
        "The todo list still holds 1 open item(s) from before.\n## Ground\n- [x] t1 read",
        false,
    );
    custom(&mut app, "reminder", "Relevant: skill://plan", true);
    let rows = flat(&app.take_commits()).join("\n");
    assert!(!rows.contains("todo_prelude"), "{rows}");
    assert!(!rows.contains("t1 read"), "{rows}");
    assert!(rows.contains("⚑ reminder Relevant: skill://plan"), "{rows}");
    Ok(())
}

/// #946 Q4: a compaction note is a custom message on the model's side, but it reads as the
/// plain host line it always was, with no `compaction_notice` source label.
#[test]
fn a_compaction_notice_reads_as_a_plain_host_line() -> TestResult {
    let mut app = app();
    let notice =
        "[compaction failed: upstream 529; history left uncompacted, the next prompt retries]";
    custom(
        &mut app,
        yi_runtime::compaction::COMPACTION_NOTICE,
        notice,
        true,
    );
    let rows = flat(&app.take_commits()).join("\n");
    assert!(rows.contains("[compaction failed: upstream 529;"), "{rows}");
    assert!(!rows.contains("compaction_notice"), "{rows}");
    Ok(())
}

/// Incident: a failed call's digest returned before the sanitizer, so the tab `diff -u` puts
/// before a header's timestamp reached the card. Producer: Apple diff (FreeBSD), exit 1.
#[test]
fn a_failed_digest_shows_no_control_character() {
    let digest = ToolCell::digest_of("bash", include_str!("fixtures/diff-u.txt"), true);
    assert_eq!(digest.as_deref(), Some("--- a.txt    2026-09-28 10:00:00"));
}

#[test]
fn a_control_character_shows_as_its_glyph_and_a_tab_as_four_spaces() {
    let raw = "a\tb\r\u{1b}[1m\u{7f}\u{85}";
    assert_eq!(
        yi_tui::transcript::show_controls(raw),
        "a    b␍␛[1m␡\u{fffd}"
    );
}

/// Invariant: whatever a body's text missed, the real terminal gets one visible cell and
/// never a control it would obey.
#[test]
fn a_control_character_in_a_cell_reaches_the_terminal_as_its_glyph() -> TestResult {
    use ratatui::backend::Backend;
    // ratatui keeps CRLF as one grapheme in one cell, so it must stay one glyph wide.
    let [tab, carriage, crlf, plain] = ["\t", "\r", "\r\n", "x"].map(|symbol| {
        let mut cell = ratatui::buffer::Cell::default();
        cell.set_symbol(symbol);
        cell
    });
    let cells = [
        (0, 0, &tab),
        (1, 0, &carriage),
        (2, 0, &crlf),
        (3, 0, &plain),
    ];
    let mut backend = yi_tui::term::ControlPictures(ratatui::backend::TestBackend::new(4, 1));
    backend.draw(cells.into_iter())?;
    backend.0.assert_buffer_lines(["␉␍␍x"]);
    Ok(())
}

/// Incident: a title holding `BEL ESC]52;…` ended OSC 2 early and had the terminal write the
/// clipboard; C1 ST (U+009C) ends an OSC on terminals that read 8-bit controls.
#[test]
fn a_window_title_carries_no_control_character() {
    let osc = yi_tui::term::window_title_osc("Fix\u{7}\u{1b}]52;c;ZWNobyBwd25lZA==\u{9c} now");
    assert_eq!(osc, "\u{1b}]2;Fix]52;c;ZWNobyBwd25lZA== now\u{7}");
}
