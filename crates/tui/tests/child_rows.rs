use std::error::Error;

use serde_json::Number;
use yi_runtime::ChildStatus;
use yi_tui::app::{App, TuiOptions};
use yi_tui::cell::Cell;
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::keymap::default_keymap;
use yi_types::event::{AgentEvent, ToolResult};
use yi_types::message::Content;
use yi_types::subagent::{ChildActivity, ChildExit, ChildId, ChildUpdate, FailClass};

use crate::common;

type TestResult = Result<(), Box<dyn Error>>;

fn app() -> App {
    App::new(
        TuiOptions {
            model: common::test_model("faux-1"),
            session_name: "child rows".to_owned(),
            cwd: "/tmp".to_owned(),
            lane: None,
            context_window: 128_000,
            session_dir: String::new(),
            keys: Vec::new(),
            initial_prompt: None,
            pace: 0,
        },
        Theme::new(ColorTier::TrueColor, true),
        default_keymap(),
        100,
    )
}

fn update(id: &str, session: &str, status: ChildStatus) -> ChildUpdate {
    ChildUpdate {
        id: ChildId(id.to_owned()),
        name: "scout".to_owned(),
        status,
        activity: ChildActivity::Waiting,
        tool_use_count: 2,
        token_count: 35,
        cost: Number::from_f64(0.012),
        answer_preview: Some("Both rows replayed".to_owned()),
        error: None,
        exit: None,
        flag: None,
        session: Some(session.to_owned()),
        role: Some("reader".to_owned()),
        task: Some("Inspect replay ordering".to_owned()),
        elapsed_ms: Some(4_000),
    }
}

fn running(id: &str, session: &str) -> ChildUpdate {
    update(id, session, ChildStatus::Running)
}

fn finished(id: &str, session: &str, exit: ChildExit) -> ChildUpdate {
    let status = if exit == ChildExit::Completed {
        ChildStatus::Completed
    } else {
        ChildStatus::Error
    };
    let mut update = update(id, session, status);
    update.exit = Some(exit);
    update.error = match exit {
        ChildExit::Failed { .. } => Some("provider refused".to_owned()),
        _ => None,
    };
    update
}

fn tool_start(call: &str) -> AgentEvent {
    AgentEvent::ToolExecutionStart {
        tool_call_id: call.to_owned(),
        tool_name: "ipython".to_owned(),
        args: serde_json::json!({"code": "h = await rlm.run('inspect')"}),
    }
}

fn tool_end(call: &str) -> AgentEvent {
    AgentEvent::ToolExecutionEnd {
        tool_call_id: call.to_owned(),
        tool_name: "ipython".to_owned(),
        result: ToolResult {
            content: vec![Content::Text {
                text: "ok".to_owned(),
                text_signature: None,
            }],
            details: serde_json::json!({"stdout": "ok\n"}),
            usage: None,
            added_tool_names: None,
            terminate: None,
        },
        is_error: false,
    }
}

fn commits(app: &mut App) -> Vec<String> {
    common::flat_trimmed(&app.take_commits())
}

fn child_rows(app: &mut App) -> Vec<String> {
    commits(app)
        .into_iter()
        .filter(|row| row.contains("subagent"))
        .collect()
}

#[test]
fn an_early_end_waits_for_its_cell_then_follows_its_start() -> TestResult {
    let mut app = app();
    app.reduce_agent(AgentEvent::AgentStart);
    app.reduce_agent(tool_start("cell-1"));
    app.adopt(&running("sub-1", "session-1"), None, Some("cell-1"));
    app.reduce_child_update(&finished("sub-1", "session-1", ChildExit::Completed));
    assert!(child_rows(&mut app).is_empty(), "nothing commits early");
    app.reduce_agent(tool_end("cell-1"));
    let committed = commits(&mut app);
    let rows: Vec<_> = committed
        .iter()
        .filter(|row| row.contains("subagent"))
        .collect();
    assert_eq!(rows.len(), 2, "{committed:?}");
    assert!(
        rows[0].starts_with("── subagent scout (sub-1) · reader · Inspect replay ordering"),
        "{committed:?}"
    );
    assert!(rows[1].starts_with("── subagent scout (sub-1) · done · 4s · 35 tokens · $0.012 · start sub-1 · Both rows replayed"), "{committed:?}");
    assert!(
        !committed.iter().any(|row| row.contains("╭─")),
        "the committed full card is replaced: {committed:?}"
    );
    Ok(())
}

#[test]
fn a_late_end_appends_after_later_parent_cells_with_a_start_back_reference() -> TestResult {
    let mut app = app();
    app.reduce_agent(AgentEvent::AgentStart);
    app.reduce_agent(tool_start("cell-1"));
    app.reduce_agent(tool_end("cell-1"));
    app.adopt(&running("sub-1", "session-1"), None, Some("cell-1"));
    let mut rows = child_rows(&mut app);
    assert_eq!(rows.len(), 1, "{rows:?}");
    app.commit_cell(&Cell::Notice {
        text: "parent moved on".to_owned(),
    });
    app.reduce_child_update(&finished("sub-1", "session-1", ChildExit::Completed));
    rows.extend(commits(&mut app));
    let start = rows
        .iter()
        .position(|row| row.contains("· reader ·"))
        .ok_or("no start")?;
    let parent = rows
        .iter()
        .position(|row| row.contains("parent moved on"))
        .ok_or("no parent")?;
    let end = rows
        .iter()
        .position(|row| row.contains("· done ·"))
        .ok_or("no end")?;
    assert!(start < parent && parent < end, "{rows:?}");
    assert!(rows[end].contains("start sub-1"), "{rows:?}");
    Ok(())
}

#[test]
fn terminal_first_rows_name_every_actual_outcome() -> TestResult {
    let cases = [
        (ChildExit::Completed, "done", "Both rows replayed"),
        (
            ChildExit::Failed {
                class: FailClass::Provider,
            },
            "failed",
            "provider refused",
        ),
        (ChildExit::Interrupted, "cancelled", ""),
        (ChildExit::Reaped, "reaped", ""),
        (ChildExit::Repossessed, "revoked", ""),
    ];
    for (index, (exit, outcome, detail)) in cases.into_iter().enumerate() {
        let id = format!("sub-{index}");
        let mut app = app();
        app.adopt(&finished(&id, "session", exit), None, None);
        let rows = child_rows(&mut app);
        assert_eq!(rows.len(), 2, "{exit:?}: {rows:?}");
        assert!(
            rows[1].contains(&format!("· {outcome} ·")),
            "{exit:?}: {rows:?}"
        );
        if !detail.is_empty() {
            assert!(rows[1].contains(detail), "{exit:?}: {rows:?}");
        }
    }
    let mut app = app();
    let mut closed = finished("sub-close", "session", ChildExit::Repossessed);
    closed.error = Some("parent closed".to_owned());
    app.adopt(&closed, None, None);
    let rows = child_rows(&mut app);
    assert!(rows[1].contains("· parent closed ·"), "{rows:?}");
    assert!(
        !rows[1].ends_with("parent closed · parent closed"),
        "{rows:?}"
    );
    Ok(())
}

#[test]
fn a_running_update_rearms_only_the_end_and_a_new_session_gets_a_new_start() -> TestResult {
    let mut app = app();
    app.adopt(&running("sub-1", "session-1"), None, None);
    app.reduce_child_update(&finished("sub-1", "session-1", ChildExit::Completed));
    app.take_commits();
    app.reduce_child_update(&running("sub-1", "session-1"));
    app.reduce_child_update(&finished("sub-1", "session-1", ChildExit::Interrupted));
    app.adopt(
        &finished("sub-1", "session-2", ChildExit::Completed),
        None,
        None,
    );
    let rows = child_rows(&mut app);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(rows[0].contains("· cancelled ·"), "{rows:?}");
    assert!(rows[1].contains("· reader ·"), "{rows:?}");
    assert!(rows[2].contains("· done ·"), "{rows:?}");
    Ok(())
}

#[test]
fn start_rows_keep_legacy_fields_absent_and_task_caps_loud() -> TestResult {
    let mut legacy_app = app();
    let mut legacy = finished("sub-old", "session-old", ChildExit::Completed);
    legacy.role = None;
    legacy.task = None;
    legacy.elapsed_ms = None;
    legacy.cost = None;
    legacy.answer_preview = None;
    legacy_app.adopt(&legacy, None, None);
    let rows = child_rows(&mut legacy_app);
    assert!(
        rows[0].starts_with("── subagent scout (sub-old)"),
        "{rows:?}"
    );
    assert!(
        rows[1].starts_with("── subagent scout (sub-old) · done · 35 tokens · start sub-old"),
        "{rows:?}"
    );

    for (id, task, capped) in [
        ("sub-240", "x".repeat(240), false),
        (
            "sub-241",
            format!(
                "{}… [kept 240 of 241 chars · cap PREVIEW_CHARS = 240 · read history://scout]",
                "x".repeat(240)
            ),
            true,
        ),
    ] {
        let mut capped_app = app();
        let mut update = running(id, "session");
        update.task = Some(task);
        capped_app.adopt(&update, None, None);
        let rows = child_rows(&mut capped_app);
        assert_eq!(
            rows[0].contains("kept 240 of 241 chars"),
            capped,
            "{rows:?}"
        );
        if capped {
            assert!(rows[0].contains("cap PREVIEW_CHARS = 240"), "{rows:?}");
            assert!(rows[0].contains("history://scout"), "{rows:?}");
        }
    }
    Ok(())
}

#[test]
fn eviction_and_roster_ticks_never_duplicate_committed_rows() -> TestResult {
    let mut app = app();
    let roster: Vec<ChildUpdate> = (0..33)
        .map(|index| {
            finished(
                &format!("sub-{index}"),
                &format!("session-{index}"),
                ChildExit::Completed,
            )
        })
        .collect();
    for update in &roster {
        app.adopt(update, None, None);
    }
    let initial = child_rows(&mut app);
    assert_eq!(initial.len(), 66, "{initial:?}");
    for update in &roster {
        app.adopt(update, None, None);
    }
    let rows = child_rows(&mut app);
    assert!(
        rows.is_empty(),
        "a repeated terminal roster duplicates rows: {rows:?}"
    );
    Ok(())
}
