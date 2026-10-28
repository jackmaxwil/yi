use std::error::Error;

use serde_json::Number;
use yi_runtime::ChildStatus;
use yi_tui::app::{App, TuiOptions};
use yi_tui::cell::{Cell, TranscriptMode};
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::keymap::default_keymap;
use yi_types::entry::Entry;
use yi_types::event::{AgentEvent, ToolResult};
use yi_types::message::Content;
use yi_types::subagent::{
    CHILD_ENTRY, ChildActivity, ChildEnded, ChildExit, ChildId, ChildSpawned, ChildTrail,
    ChildUpdate, FailClass,
};

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

#[tokio::test]
async fn a_host_caps_the_journaled_and_committed_start_task() -> TestResult {
    use std::sync::Arc;
    use yi_runtime::session_store::{CreateOptions, JsonlRepo, SessionRepo, lock_session};
    use yi_runtime::{
        AgentSession, ProviderStream, SessionConfig, SubagentHost, SubagentHostOptions,
    };

    let dir = crate::scratch::Scratch::new("yi-tui-child-task-cap")?;
    let mut repo = JsonlRepo::new(dir.join("sessions"), "/tmp");
    let parent = repo.create(CreateOptions::default())?;
    let root_file = lock_session(&parent)
        .file_path()
        .cloned()
        .ok_or("no root file")?;
    let store = Arc::clone(&parent);
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        provider: Arc::new(ProviderStream::new(None)),
        depth: 0,
        max_depth: 1,
        max_children: 1,
        parent_session_dir: dir.join("children"),
        defaults: Arc::new(|| {
            (
                common::test_model("faux-1"),
                yi_types::model::Effort::Medium,
            )
        }),
        factory: Arc::new(|build| {
            Ok(AgentSession::new(
                SessionConfig {
                    system_prompt: String::new(),
                    model: build.model,
                    thinking_level: build.thinking,
                    tool_execution: yi_runtime::ExecutionMode::Sequential,
                },
                Arc::new(ProviderStream::new(None)),
            ))
        }),
        notice: Arc::new(|_, _| {}),
        events: tokio::sync::broadcast::channel(8).0,
        parent_messages: Arc::new(Vec::new),
        cwd: dir.to_path_buf(),
        home: dir.join("home"),
        lane_slots: 1,
        report: Arc::new(|_, _| {}),
        attribute: Arc::new(|_| {}),
        store: Arc::new(move || Some(Arc::clone(&store))),
        plans_dir: dir.join(".yi/plans"),
        family_live: yi_runtime::fetch::KernelServiceMap::new(),
    }));
    let args = serde_json::json!({"name": "scout", "role": "root"});
    host.spawn(
        "x".repeat(241),
        args.as_object().cloned().unwrap_or_default(),
    )?;
    let entries = recorded_entries(&root_file.to_string_lossy())?;
    let spawned = entries.iter().find_map(|entry| match entry {
        Entry::Custom {
            data: Some(data), ..
        } => match serde_json::from_value(data.clone()).ok()? {
            ChildTrail::Spawned(line) => Some(line),
            _ => None,
        },
        _ => None,
    });
    let task = spawned
        .and_then(|line| line.task)
        .ok_or("no journaled task")?;
    let cut = "[kept 240 of 241 chars · cap PREVIEW_CHARS = 240 · read history://scout]";
    assert!(task.contains(cut), "{task}");
    let rows = replayed_children(&mut app(), &entries);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].contains(cut), "{rows:?}");
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

const ENRICHED_SESSION: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/sessions/01a00000.jsonl"
);
const LEGACY_SESSION: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/sessions/01a13357.jsonl"
);

fn recorded_entries(path: &str) -> Result<Vec<Entry>, Box<dyn Error>> {
    std::fs::read_to_string(path)?
        .lines()
        .filter(|line| line.contains("\"kind\":\"entry\""))
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}

fn replayed_children(app: &mut App, entries: &[Entry]) -> Vec<String> {
    app.replay_entries(entries);
    commits(app)
        .into_iter()
        .filter(|row| row.contains("── subagent"))
        .collect()
}

fn custom(seq: u64, trail: &ChildTrail) -> Result<Entry, Box<dyn Error>> {
    Ok(Entry::Custom {
        id: format!("child-{seq}"),
        custom_type: CHILD_ENTRY.to_owned(),
        data: Some(serde_json::to_value(trail)?),
        parent_id: None,
        seq,
        timestamp: seq,
    })
}

fn trail_parts() -> Result<(ChildSpawned, ChildEnded), Box<dyn Error>> {
    let entries = recorded_entries(ENRICHED_SESSION)?;
    let spawn = entries.iter().find_map(|entry| match entry {
        Entry::Custom {
            data: Some(data), ..
        } => match serde_json::from_value(data.clone()).ok()? {
            ChildTrail::Spawned(line) => Some(line),
            _ => None,
        },
        _ => None,
    });
    let end = entries.iter().find_map(|entry| match entry {
        Entry::Custom {
            data: Some(data), ..
        } => match serde_json::from_value(data.clone()).ok()? {
            ChildTrail::Ended(line) => Some(line),
            _ => None,
        },
        _ => None,
    });
    Ok((
        spawn.ok_or("no recorded spawn")?,
        end.ok_or("no recorded end")?,
    ))
}

fn recorded_update(status: ChildStatus) -> Result<ChildUpdate, Box<dyn Error>> {
    let running = status == ChildStatus::Running;
    let (spawn, end) = trail_parts()?;
    Ok(ChildUpdate {
        id: spawn.id,
        name: spawn.name,
        status,
        activity: ChildActivity::Waiting,
        tool_use_count: 0,
        token_count: end.tokens,
        cost: end.cost,
        answer_preview: end.answer_preview,
        error: end.error,
        exit: (!running).then_some(end.exit),
        flag: None,
        session: Some(spawn.session),
        role: spawn.role,
        task: spawn.task,
        elapsed_ms: end.elapsed_ms,
    })
}

#[test]
fn replay_builds_enriched_and_reduced_legacy_rows() -> TestResult {
    let enriched = replayed_children(&mut app(), &recorded_entries(ENRICHED_SESSION)?);
    assert_eq!(enriched.len(), 2, "{enriched:?}");
    assert!(
        enriched[0].contains("· reader · Inspect replay ordering"),
        "{enriched:?}"
    );
    assert!(
        enriched[1].contains(
            "· done · 0ms · 0 tokens · start sub-5c90b1bb · The child has two persistent rows."
        ),
        "{enriched:?}"
    );
    let legacy = replayed_children(&mut app(), &recorded_entries(LEGACY_SESSION)?);
    assert_eq!(legacy.len(), 2, "{legacy:?}");
    assert!(!legacy[0].contains("· reader ·"), "{legacy:?}");
    assert!(!legacy[1].contains("elapsed"), "{legacy:?}");
    Ok(())
}

#[test]
fn replayed_rows_match_live_bytes_in_every_mode_and_after_resize() -> TestResult {
    for mode in [
        TranscriptMode::Normal,
        TranscriptMode::Thinking,
        TranscriptMode::Verbose,
    ] {
        let mut live = app();
        while live.mode() != mode {
            live.cycle_mode();
        }
        live.reduce_agent(tool_start("call-child"));
        live.reduce_agent(tool_end("call-child"));
        live.adopt(
            &recorded_update(ChildStatus::Running)?,
            None,
            Some("call-child"),
        );
        live.reduce_child_update(&recorded_update(ChildStatus::Completed)?);
        let live_rows = child_rows(&mut live);
        let mut replay = app();
        while replay.mode() != mode {
            replay.cycle_mode();
        }
        let replay_rows = replayed_children(&mut replay, &recorded_entries(ENRICHED_SESSION)?);
        assert_eq!(live_rows, replay_rows, "{mode:?}");
        replay.set_width(60);
        let resized = common::flat_trimmed(&replay.reflowed(100));
        let resized: Vec<_> = resized
            .into_iter()
            .filter(|row| row.contains("── subagent"))
            .collect();
        assert_eq!(resized.len(), 2, "{mode:?}: {resized:?}");
        assert!(
            resized[0]
                .starts_with("── subagent scout (sub-5c90b1bb) · reader · Inspect replay ordering"),
            "{mode:?}: resize"
        );
        assert!(resized[1].starts_with("── subagent scout (sub-5c90b1bb) · done · 0ms · 0 tokens · start sub-5c90b1bb · The child has two persistent rows."), "{mode:?}: resize");
    }
    Ok(())
}

#[test]
fn replay_commits_every_ending_and_matches_the_live_rows() -> TestResult {
    let (spawn, first_end) = trail_parts()?;
    let mut second_end = first_end.clone();
    second_end.exit = ChildExit::Interrupted;
    second_end.error = Some("stopped".to_owned());
    let mut live = app();
    live.adopt(&recorded_update(ChildStatus::Running)?, None, None);
    live.reduce_child_update(&recorded_update(ChildStatus::Completed)?);
    live.reduce_child_update(&recorded_update(ChildStatus::Running)?);
    let mut second_update = recorded_update(ChildStatus::Error)?;
    second_update.exit = Some(ChildExit::Interrupted);
    second_update.error = second_end.error.clone();
    live.reduce_child_update(&second_update);
    let live_rows = child_rows(&mut live);
    assert_eq!(live_rows.len(), 3, "{live_rows:?}");

    let entries = [
        custom(1, &ChildTrail::Spawned(spawn))?,
        custom(2, &ChildTrail::Ended(first_end))?,
        custom(3, &ChildTrail::Ended(second_end))?,
    ];
    let replay_rows = replayed_children(&mut app(), &entries);
    assert_eq!(replay_rows, live_rows);
    Ok(())
}

#[test]
fn replay_flushes_an_unresolved_antecedent_at_eof() -> TestResult {
    let (spawn, end) = trail_parts()?;
    let entries = [
        custom(1, &ChildTrail::Spawned(spawn))?,
        custom(2, &ChildTrail::Ended(end))?,
    ];
    let rows = replayed_children(&mut app(), &entries);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(rows[0].contains("· reader ·"), "{rows:?}");
    assert!(rows[1].contains("· done ·"), "{rows:?}");
    Ok(())
}

#[test]
fn replayed_rows_reach_a_nonzero_vt100_offset_once() -> TestResult {
    use std::io::Write;
    let mut app = app();
    let entries = recorded_entries(ENRICHED_SESSION)?;
    app.replay_entries(&entries);
    app.replay_entries(&entries);
    let mut backend = common::VT100Backend::with_scrollback(140, 36, 200);
    backend.write_all(b"\n\n\n\n\n\n\n\n")?;
    let mut terminal = yi_tui::terminal::Terminal::new(backend, 3)?;
    yi_tui::render::draw(&mut app, &mut terminal, None);
    let rows: Vec<String> = (0..36)
        .map(|row| terminal.backend().row_text(row))
        .filter(|row| row.contains("── subagent"))
        .collect();
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert!(
        rows[0].starts_with("── subagent scout (sub-5c90b1bb) · reader · Inspect replay ordering"),
        "{rows:?}"
    );
    assert!(rows[1].starts_with("── subagent scout (sub-5c90b1bb) · done · 0ms · 0 tokens · start sub-5c90b1bb · The child has two persistent rows."), "{rows:?}");
    Ok(())
}
