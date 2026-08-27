mod common;

use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::VT100Backend;

use serde_json::{Map, Number};
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, SubagentHost, SubagentHostOptions};
use yi_tui::app::{App, TuiOptions};
use yi_tui::cell::Cell;
use yi_tui::colors::{ColorTier, Theme};
use yi_tui::keymap::default_keymap;
use yi_types::message::StopReason;
use yi_types::model::{Model, ModelCost};

type TestResult = Result<(), Box<dyn Error>>;

fn faux_model() -> Model {
    let zero = || Number::from(0u64);
    Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 128_000,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn faux_session(reply: &str) -> AgentSession {
    let provider = Arc::new(ProviderStream::new(None, None));
    provider.queue_faux(vec![yi_runtime::faux::faux_assistant_message(
        vec![yi_runtime::faux::faux_text(reply)],
        StopReason::Stop,
    )]);
    AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: yi_runtime::ExecutionMode::Sequential,
        },
        provider,
    )
}

fn options() -> TuiOptions {
    TuiOptions {
        model_label: "faux-1".to_owned(),
        session_name: "e2e".to_owned(),
        cwd: "/tmp".to_owned(),
        context_window: 128_000,
        session_dir: String::new(),
        keys: Vec::new(),
        initial_prompt: None,
    }
}

fn app() -> App {
    App::new(
        options(),
        Theme::new(ColorTier::TrueColor, true),
        default_keymap(),
        80,
    )
}

fn flat_lines(lines: &[ratatui::text::Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn faux_turn_renders_user_and_assistant_cells() -> TestResult {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut app = app();
    runtime.block_on(async {
        let session = faux_session("faux: pong");
        let mut events = session.subscribe();
        session.prompt("ping").map_err(|e| format!("{e:?}"))?;
        loop {
            let event = events
                .recv()
                .await
                .map_err(|e| format!("event stream died: {e}"))?;
            let done = matches!(event, yi_types::event::AgentEvent::AgentEnd { .. });
            app.reduce_agent(event);
            if done {
                break;
            }
        }
        Ok::<(), String>(())
    })?;
    let committed = flat_lines(&app.take_commits());
    assert!(
        committed.iter().any(|l| l.contains("› ping")),
        "user cell committed: {committed:?}"
    );
    assert!(
        committed.iter().any(|l| l.contains("faux: pong")),
        "assistant markdown committed: {committed:?}"
    );
    Ok(())
}

#[test]
fn commit_lines_land_in_a_real_vt100_screen() -> TestResult {
    let backend = VT100Backend::with_scrollback(80, 24, 200);
    let mut terminal = yi_tui::terminal::Terminal::new(backend, 6)?;
    let theme = Theme::new(ColorTier::TrueColor, true);
    let cell = Cell::User {
        text: "hello vt100".to_owned(),
    };
    yi_tui::term::commit_lines(
        &mut terminal,
        cell.lines(80, &theme, yi_tui::cell::TranscriptMode::Normal, 0),
    )?;
    let contents = terminal.backend().contents();
    assert!(
        contents.contains("hello vt100"),
        "insert_before must land above the viewport on a real VT parser: {contents}"
    );
    Ok(())
}

#[test]
fn subagent_task_cell_focus_and_back() -> TestResult {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let dir = std::env::temp_dir().join(format!("yi-tui-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        depth: 0,
        max_depth: 1,
        max_children: 4,
        parent_session_dir: dir.clone(),
        default_model: faux_model(),
        factory: Arc::new(|_model, _thinking, _dir| Ok(faux_session("child answer"))),
        notice: Arc::new(|_notice| {}),
        attribute: Arc::new(|_usage| {}),
    }));
    let mut app = app();
    runtime.block_on(async {
        host.spawn("trace the render path".to_owned(), Map::new())
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let children = host.children_view();
            app.sync_children(&children);
            if children
                .iter()
                .any(|c| c.status != yi_runtime::ChildStatus::Running)
            {
                break;
            }
            if Instant::now() > deadline {
                return Err("child never finished".to_owned());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        Ok::<(), String>(())
    })?;
    let committed = flat_lines(&app.take_commits());
    assert!(
        committed
            .iter()
            .any(|l| l.contains("Task —") || l.contains("Task —")),
        "finished child commits a task cell: {committed:?}"
    );
    assert!(
        committed.iter().any(|l| l.contains("toolcalls")),
        "task cell carries the toolcall counter line: {committed:?}"
    );

    let child_id = host
        .children_view()
        .first()
        .map(|c| c.child_id.clone())
        .ok_or("child missing")?;
    app.set_focus(Some(child_id.clone()));
    let focus_commits = flat_lines(&app.take_commits());
    assert!(
        focus_commits.iter().any(|l| l.contains("subagent")),
        "focusing prints the entry rule: {focus_commits:?}"
    );
    assert_eq!(app.focused(), Some(child_id.as_str()));
    app.set_focus(None);
    let back_commits = flat_lines(&app.take_commits());
    assert!(
        back_commits.iter().any(|l| l.contains("back to parent")),
        "unfocusing prints the closing rule: {back_commits:?}"
    );
    assert_eq!(app.focused(), None);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[test]
fn draw_paints_inside_the_inline_viewport_offset() -> TestResult {
    let mut backend = VT100Backend::with_scrollback(80, 24, 200);
    {
        use std::io::Write;
        backend.write_all(b"\n\n\n\n\n\n\n\n")?;
    }
    let mut terminal = yi_tui::terminal::Terminal::new(backend, 6)?;
    let mut app = app();
    yi_tui::render::draw(&mut app, &mut terminal, None);
    let contents = terminal.backend().contents();
    assert!(
        contents.contains("faux-1"),
        "the status row must land inside the offset viewport area \
         (a rect anchored at y=0 renders nowhere): {contents}"
    );
    Ok(())
}

#[test]
fn external_editor_replaces_the_draft() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-tui-editor-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let editor = dir.join("fake-editor.sh");
    std::fs::write(
        &editor,
        "#!/bin/sh\nprintf 'edited elsewhere\\n' > \"$1\"\n",
    )?;
    let mut permissions = std::fs::metadata(&editor)?.permissions();
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o755);
    }
    std::fs::set_permissions(&editor, permissions)?;
    // SAFETY-free: the test process is the only reader of EDITOR here.
    unsafe { std::env::set_var("EDITOR", &editor) };

    let mut app = app();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = yi_tui::terminal::Terminal::new(backend, 6)?;
    app.open_editor();
    yi_tui::editor::process_pending_editor(&mut app, &mut terminal, false);

    assert_eq!(app.composer_text(), "edited elsewhere");
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

fn tool_start(id: &str) -> yi_types::event::AgentEvent {
    yi_types::event::AgentEvent::ToolExecutionStart {
        tool_call_id: id.to_owned(),
        tool_name: "read".to_owned(),
        args: serde_json::json!({"i": format!("Reading {id}")}),
    }
}

#[test]
fn viewport_height_follows_the_live_region_and_stays_bottom_anchored() -> TestResult {
    let mut backend = VT100Backend::with_scrollback(80, 24, 200);
    {
        use std::io::Write;
        // A real session starts with the shell prompt on the last row; the
        // inline viewport must anchor there, not at row 0.
        backend.write_all(&b"\n".repeat(23))?;
    }
    let mut terminal = yi_tui::terminal::Terminal::new(backend, 4)?;
    let mut app = app();

    yi_tui::render::draw(&mut app, &mut terminal, None);
    let idle_height = terminal.viewport_area().height;
    assert_eq!(idle_height, 4, "idle live region is composer box + status");
    assert!(
        terminal.backend().row_text(23).contains("faux-1"),
        "the status row must sit on the last screen row when idle, not {} rows above it: {:?}",
        23 - terminal.viewport_area().bottom().saturating_sub(1),
        terminal.backend().row_text(23)
    );

    for id in ["a", "b", "c", "d", "e"] {
        app.reduce_agent(tool_start(id));
    }
    yi_tui::render::draw(&mut app, &mut terminal, None);
    assert!(
        terminal.viewport_area().height > idle_height,
        "five live tool cells must grow the viewport past {idle_height} rows"
    );
    assert_eq!(
        terminal.viewport_area().bottom(),
        24,
        "growth scrolls the rows above up; the viewport stays on the bottom row"
    );
    assert!(
        terminal.backend().row_text(23).contains("faux-1"),
        "status row after growth: {:?}",
        terminal.backend().row_text(23)
    );
    Ok(())
}

#[test]
fn resizing_the_window_leaves_one_composer_border() -> TestResult {
    for (label, steps) in [
        (
            "grow",
            vec![(90_u16, 30_u16), (100, 40), (110, 50), (122, 66)],
        ),
        ("shrink", vec![(110, 50), (100, 40), (90, 30), (80, 24)]),
    ] {
        let mut backend = VT100Backend::with_scrollback(122, 66, 200);
        {
            use std::io::Write;
            backend.write_all(&b"\n".repeat(65))?;
        }
        let mut terminal = yi_tui::terminal::Terminal::new(backend, 4)?;
        let mut app = app();
        app.set_width(122);
        app.commit_cell(&yi_tui::cell::Cell::User {
            text: "a committed transcript line".to_owned(),
        });
        yi_tui::render::draw(&mut app, &mut terminal, None);

        for (width, height) in steps {
            // A real emulator reflows on resize: rows move and the cursor moves
            // with them. vt100's `set_size` does not reflow, so scroll the
            // screen and move the cursor by the same amount by hand.
            {
                use ratatui::backend::Backend;
                use std::io::Write;
                let before = terminal.backend_mut().get_cursor_position()?;
                write!(terminal.backend_mut(), "\x1b[2S")?;
                write!(
                    terminal.backend_mut(),
                    "\x1b[{};1H",
                    before.y.saturating_sub(2) + 1
                )?;
            }
            terminal.backend_mut().resize(width, height);
            app.set_width(usize::from(width));
            yi_tui::render::draw(&mut app, &mut terminal, None);
        }

        let contents = terminal.backend().contents();
        let borders = contents.matches('\u{256d}').count();
        assert_eq!(
            borders, 1,
            "{label}: every resize must repaint one composer box, \
             not stack a stale one per step:\n{contents}"
        );
    }
    Ok(())
}

#[test]
fn a_running_turn_aims_the_mark_at_the_orb() -> TestResult {
    let backend = VT100Backend::with_scrollback(80, 24, 200);
    let mut terminal = yi_tui::terminal::Terminal::new(backend, 4)?;
    let mut app = app();
    app.set_kitty(true);

    yi_tui::render::draw(&mut app, &mut terminal, None);
    assert_eq!(
        app.logo_target(),
        0.0,
        "at rest the mark holds the wordmark"
    );

    app.reduce_agent(yi_types::event::AgentEvent::AgentStart);
    yi_tui::render::draw(&mut app, &mut terminal, None);
    assert_eq!(
        app.logo_target(),
        1.0,
        "a running turn aims the morph at the orb"
    );

    app.reduce_agent(yi_types::event::AgentEvent::AgentEnd { messages: vec![] });
    yi_tui::render::draw(&mut app, &mut terminal, None);
    assert_eq!(
        app.logo_target(),
        0.0,
        "the turn ending aims it back at the wordmark"
    );
    Ok(())
}
