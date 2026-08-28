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
            model_label: "faux-1".to_owned(),
            session_name: "cells".to_owned(),
            cwd: "/tmp".to_owned(),
            context_window: 128_000,
            session_dir: String::new(),
            keys: Vec::new(),
            initial_prompt: None,
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
    app.reduce_agent(AgentEvent::ToolExecutionEnd {
        tool_call_id: id.to_owned(),
        tool_name: tool.to_owned(),
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
        is_error: false,
    });
}

/// Eight rows of looking crowds out the answer that follows it; codex groups
/// the run under one bullet with a verb column.
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
    assert!(joined.contains("→ read src/lib.rs"), "{joined}");
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

/// opencode's rule: a blank separates blocks, and a run of one-line calls packs
/// flush. Both halves matter — always blank is as wrong as never blank.
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
