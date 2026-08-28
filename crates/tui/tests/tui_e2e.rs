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
        events: tokio::sync::broadcast::channel(64).0,
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
                .any(|c| c.update.status != yi_runtime::ChildStatus::Running)
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
        .map(|c| c.update.id.as_str().to_owned())
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

        // D47: a drag leaves intermediate rows on screen on purpose — the
        // rebuild is trailing-debounced so it runs once at the settled width.
        // Wait it out, then draw: that is the state the user is left looking at.
        std::thread::sleep(yi_tui::reflow::REFLOW_DEBOUNCE + Duration::from_millis(10));
        yi_tui::render::draw(&mut app, &mut terminal, None);

        let contents = terminal.backend().contents();
        let borders = contents.matches('\u{256d}').count();
        assert_eq!(
            borders, 1,
            "{label}: once a resize settles, the rebuilt transcript carries one \
             composer box, not a stale one per drag step:\n{contents}"
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

/// U13 commits each stable paragraph to scrollback mid-stream, so the row the
/// mark used to occupy — the top of the viewport — is the commit boundary, not
/// the top of the answer. Rendered there it sat between the committed prose and
/// the streaming tail; a reader saw the agent's own mark spliced into the middle
/// of its answer, once per paragraph.
#[test]
fn the_mark_trails_the_streaming_tail_instead_of_splitting_the_answer() -> TestResult {
    let backend = VT100Backend::with_scrollback(80, 24, 200);
    let mut terminal = yi_tui::terminal::Terminal::new(backend, 4)?;
    let mut app = app();
    app.reduce_agent(yi_types::event::AgentEvent::AgentStart);

    // Two paragraphs: the first is stable and commits, the second is the tail
    // still streaming under it.
    let streaming = "Committed paragraph.\n\nTail paragraph still streaming";
    let message = yi_types::message::AgentMessage::Assistant {
        content: vec![yi_types::message::Content::Text {
            text: streaming.to_owned(),
            text_signature: None,
        }],
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: yi_types::message::Usage::zero(),
        stop_reason: yi_types::message::StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    };
    app.reduce_agent(yi_types::event::AgentEvent::MessageUpdate {
        message: message.clone(),
        assistant_message_event: yi_types::event::AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: String::new(),
            partial: message,
        },
    });
    yi_tui::render::draw(&mut app, &mut terminal, None);

    let contents = terminal.backend().contents();
    let row_of = |needle: &str| {
        contents
            .lines()
            .position(|line| line.contains(needle))
            .ok_or_else(|| format!("{needle:?} is not on screen:\n{contents}"))
    };
    let committed = row_of("Committed paragraph.")?;
    let tail = row_of("Tail paragraph still streaming")?;
    let mark = row_of("interrupt")?;

    assert!(
        committed < tail,
        "the committed prefix stays above the tail:\n{contents}"
    );
    assert!(
        tail < mark,
        "the mark must sit after every rendered line of the answer, not between \
         the committed half and the streaming half:\n{contents}"
    );
    Ok(())
}

/// `resize_viewport` clamps the viewport to the screen and `put` silently drops
/// what no longer fits, so an unbudgeted live tail evicted the rows drawn after
/// it. The status line went first, then the composer — the row the user types
/// into, gone with no indication anything had been cut.
#[test]
fn a_short_screen_keeps_the_composer_and_status_under_a_long_tail() -> TestResult {
    for height in [8u16, 10, 14, 24] {
        let backend = VT100Backend::with_scrollback(80, height, 200);
        let mut terminal = yi_tui::terminal::Terminal::new(backend, 4)?;
        let mut app = app();
        app.set_rows(usize::from(height));
        app.reduce_agent(yi_types::event::AgentEvent::AgentStart);
        let body = (1..=40)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let message = yi_types::message::AgentMessage::Assistant {
            content: vec![yi_types::message::Content::Text {
                text: body,
                text_signature: None,
            }],
            api: String::new(),
            provider: String::new(),
            model: String::new(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: yi_types::message::Usage::zero(),
            stop_reason: yi_types::message::StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 0,
        };
        app.reduce_agent(yi_types::event::AgentEvent::MessageUpdate {
            message: message.clone(),
            assistant_message_event: yi_types::event::AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: String::new(),
                partial: message,
            },
        });
        yi_tui::render::draw(&mut app, &mut terminal, None);

        let contents = terminal.backend().contents();
        assert!(
            contents.contains('╰'),
            "the composer must survive a {height}-row screen:\n{contents}"
        );
        assert!(
            contents.contains("0% of 128K"),
            "the status line must survive a {height}-row screen:\n{contents}"
        );
    }
    Ok(())
}

/// T13: the permission layer builds a diff for every mutating call, and the
/// prompt is where the user reads it. The description arrives newline-joined
/// and `wrap_line` has no newline handling, so the whole patch used to flatten
/// into one span and get cut to three rows of mangled prose.
#[test]
fn the_approval_prompt_renders_its_diff_as_a_diff() -> TestResult {
    use yi_tui::approval::ApprovalView;
    use yi_tui::popup::BottomView;

    let description = [
        "edit crates/tui/src/cell.rs",
        "--- a/crates/tui/src/cell.rs",
        "+++ b/crates/tui/src/cell.rs",
        "@@ -208,3 +208,4 @@",
        " fn glyph(tool: &str) -> char {",
        "-    match tool {",
        "+    match tool.trim() {",
    ]
    .join("\n");
    let view = ApprovalView::new("write".to_owned(), description);
    let theme = Theme::new(ColorTier::TrueColor, true);
    let rendered = flat_lines(&view.lines(80, &theme));
    let joined = rendered.join("\n");

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("-    match tool {")),
        "the removed line is what the decision turns on:\n{joined}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("+    match tool.trim() {")),
        "the added line too:\n{joined}"
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("@@ -208,3 +208,4 @@")),
        "the hunk header locates the change:\n{joined}"
    );
    assert!(
        !joined.contains("--- a/"),
        "the git path headers repeat the title and cost two rows:\n{joined}"
    );
    Ok(())
}

/// A kitty placement scrolls with the text under it, and a resize reflows that
/// text — but the viewport rect often does not move (the window is neither
/// bottom-aligned nor overflowing), so the app computed the same cell, skipped
/// the re-emit, and the mark sat wherever the emulator had left it. At rest the
/// morph is settled, so nothing re-armed the redraw until the next turn.
#[test]
fn a_resize_forces_the_mark_to_be_placed_again() -> TestResult {
    use ratatui::crossterm::event::Event as CtEvent;

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = app();
    app.set_kitty(true);
    assert!(
        !app.take_orb_stale(),
        "nothing is stale before a resize arrives"
    );

    yi_tui::input::handle_terminal_event(&mut app, &tx, CtEvent::Resize(100, 30));
    assert!(
        app.take_orb_stale(),
        "a resize must force the next tick to place the image again — the app's \
         own placement can be unchanged while the emulator has moved the image"
    );
    assert!(
        !app.take_orb_stale(),
        "the flag is consumed, so one resize costs exactly one re-place"
    );
    Ok(())
}

/// D47: a width change makes every wrapped row in scrollback wrong. The rebuild
/// clears scrollback and the visible screen and re-emits the retained transcript
/// at the new width — it does not repaint a window's worth of fresh wrapping
/// over rows the emulator has already reflowed, which is what left the same
/// prose on screen twice at two different widths.
#[test]
fn a_settled_resize_rebuilds_the_transcript_at_the_new_width() -> TestResult {
    let backend = VT100Backend::with_scrollback(100, 20, 400);
    let mut terminal = yi_tui::terminal::Terminal::new(backend, 4)?;
    let mut app = app();
    app.set_rows(20);
    app.set_width(100);
    app.commit_cell(&yi_tui::cell::Cell::Notice {
        text: "a line long enough to wrap at sixty columns but not at one hundred columns"
            .to_owned(),
    });
    yi_tui::render::draw(&mut app, &mut terminal, None);
    let wide = terminal.backend().contents();
    assert_eq!(
        wide.matches("a line long enough").count(),
        1,
        "one copy at the original width:\n{wide}"
    );

    terminal.backend_mut().resize(60, 20);
    app.set_width(60);
    yi_tui::render::draw(&mut app, &mut terminal, None);
    // Trailing debounce: the rebuild belongs to the settled width, not to any
    // width the drag passed through.
    std::thread::sleep(yi_tui::reflow::REFLOW_DEBOUNCE + std::time::Duration::from_millis(10));
    yi_tui::render::draw(&mut app, &mut terminal, None);

    let narrow = terminal.backend().contents();
    assert_eq!(
        narrow.matches("a line long enough").count(),
        1,
        "the rebuild clears before it re-emits, so the transcript appears once, \
         not once per width it has been rendered at:\n{narrow}"
    );
    assert!(
        narrow.contains("a line long enough to wrap at"),
        "and the text survives the rebuild:\n{narrow}"
    );
    Ok(())
}
