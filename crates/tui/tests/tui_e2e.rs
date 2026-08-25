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
    let mut terminal = ratatui::Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(6),
        },
    )?;
    let theme = Theme::new(ColorTier::TrueColor, true);
    let cell = Cell::User {
        text: "hello vt100".to_owned(),
    };
    yi_tui::term::commit_lines(&mut terminal, cell.lines(80, &theme, false, 0))?;
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
    let mut terminal = ratatui::Terminal::with_options(
        backend,
        ratatui::TerminalOptions {
            viewport: ratatui::Viewport::Inline(6),
        },
    )?;
    let mut app = app();
    yi_tui::app::draw(&mut app, &mut terminal, None);
    let contents = terminal.backend().contents();
    assert!(
        contents.contains("faux-1"),
        "the status row must land inside the offset viewport area \
         (a rect anchored at y=0 renders nowhere): {contents}"
    );
    Ok(())
}
