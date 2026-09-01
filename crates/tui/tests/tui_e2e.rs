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
        model: faux_model(),
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

/// The tinted rows are built from styles, not text, so a diff that reads
/// correctly in a `Vec<Line>` can still land as nothing on a real screen —
/// especially below a nonzero viewport offset, where a rect anchored at y=0
/// paints off-screen.
#[test]
fn a_diff_body_lands_on_a_real_screen_below_a_viewport_offset() -> TestResult {
    let mut backend = VT100Backend::with_scrollback(80, 24, 200);
    {
        use std::io::Write;
        backend.write_all(b"\n\n\n\n\n\n\n\n")?;
    }
    let mut terminal = yi_tui::terminal::Terminal::new(backend, 6)?;
    let theme = Theme::new(ColorTier::TrueColor, true);
    let cell = Cell::Tool(yi_tui::cell::ToolCell {
        name: "edit".to_owned(),
        call_id: String::new(),
        intent: None,
        status: yi_tui::cell::ToolStatus::Done,
        summary: yi_tui::cell::ToolCell::summary_of("edit", "src/lib.rs"),
        digest: Some("updated; first change at line 2".to_owned()),
        preview: Vec::new(),
        elapsed_ms: 0,
        calls: 1,
        details: serde_json::json!({
            "patch": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,3 @@\n one\n-let total = a + b;\n+let total = a - b;\n three\n",
            "added": 1,
            "removed": 1,
        }),
    });
    yi_tui::term::commit_lines(
        &mut terminal,
        cell.lines(80, &theme, yi_tui::cell::TranscriptMode::Normal, 0),
    )?;
    let contents = terminal.backend().contents();
    assert!(
        contents.contains("let total = a - b;") && contents.contains("let total = a + b;"),
        "both sides of the change must reach the screen: {contents}"
    );
    assert!(
        contents.contains("+1 -1"),
        "the stats ride the digest row: {contents}"
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
        cwd: dir.clone(),
        defaults: Arc::new(|| (faux_model(), yi_types::model::Effort::Medium)),
        factory: Arc::new(|_build| Ok(faux_session("child answer"))),
        notice: Arc::new(|_notice| {}),
        events: tokio::sync::broadcast::channel(64).0,
        parent_messages: Arc::new(Vec::new),
        report: Arc::new(|_message| {}),
        attribute: Arc::new(|_usage| {}),
        store: Arc::new(|| None),
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

fn streamed(body: &str) -> yi_types::event::AgentEvent {
    let message = yi_types::message::AgentMessage::Assistant {
        content: vec![yi_types::message::Content::Text {
            text: body.to_owned(),
            text_signature: None,
        }],
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: yi_types::message::Usage::zero(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    };
    yi_types::event::AgentEvent::MessageUpdate {
        message: message.clone(),
        assistant_message_event: yi_types::event::AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: String::new(),
            partial: message,
        },
    }
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

/// The live region is bottom-anchored and its height follows its content, so a
/// streamed line moved the anchor — and every anchor move erased from there to
/// the end of the screen and repainted the rows the reader was already reading.
#[test]
fn a_streamed_line_is_written_as_an_append() -> TestResult {
    let mut backend = VT100Backend::with_scrollback(80, 24, 200);
    {
        use std::io::Write;
        backend.write_all(&b"\n".repeat(23))?;
    }
    let mut terminal = yi_tui::terminal::Terminal::new(backend, 4)?;
    let mut app = app();
    app.set_rows(24);
    app.reduce_agent(yi_types::event::AgentEvent::AgentStart);

    let mut body = String::new();
    for n in 1..=8 {
        body.push_str(&format!("- item {n} of the answer\n"));
        app.reduce_agent(streamed(&body));
        let _ = terminal.backend_mut().take_written();
        yi_tui::render::draw(&mut app, &mut terminal, None);
        let written = terminal.backend_mut().take_written();
        if n == 1 {
            // The turn's first frame lays the region out; there is nothing to keep.
            continue;
        }
        assert!(
            !written.contains("\u{1b}[J") && !written.contains("\u{1b}[0J"),
            "item {n} erased the screen from the anchor down: {written:?}"
        );
        assert!(
            !written.contains("item 1 of the answer") && !written.contains("faux-1"),
            "item {n} rewrote rows that had not changed: {written:?}"
        );
        assert!(
            written.contains("item") && written.len() < 400,
            "item {n} must cost one row of writes, not a repaint of {}: {written:?}",
            terminal.viewport_area().height
        );
    }

    let contents = terminal.backend().contents();
    for n in 1..=8 {
        assert!(
            contents.contains(&format!("item {n} of the answer")),
            "item {n} must still be on screen:\n{contents}"
        );
    }
    assert!(
        contents.contains('╰') && contents.contains("faux-1"),
        "composer and status survive the appends:\n{contents}"
    );
    assert_eq!(
        terminal.viewport_area().bottom(),
        24,
        "the region stays anchored to the bottom row:\n{contents}"
    );

    // Past the live tail's limit each streamed line commits one, so the region
    // shrinks back and takes the erase path the append never took. Pinned here
    // so the append's measured win stays attached to the phase it was measured in.
    for n in 9..=20 {
        body.push_str(&format!("- item {n} of the answer\n"));
        app.reduce_agent(streamed(&body));
        yi_tui::render::draw(&mut app, &mut terminal, None);
    }
    let capped = terminal.viewport_area().height;
    for n in 21..=32 {
        body.push_str(&format!("- item {n} of the answer\n"));
        app.reduce_agent(streamed(&body));
        let _ = terminal.backend_mut().take_written();
        yi_tui::render::draw(&mut app, &mut terminal, None);
    }
    let written = terminal.backend_mut().take_written();
    assert!(
        terminal.viewport_area().height <= capped,
        "the live tail is capped at {capped}; the region must stop following the answer"
    );
    assert!(
        written.contains("\u{1b}[J"),
        "a saturated tail still repaints — if this stopped being true the append \
         now covers the commit path too, and the 0.98.0 row must say so: {written:?}"
    );
    Ok(())
}

/// A screen short enough to leave the live region one row puts the whole floor
/// under the scroll, and a one-row region is an invalid DECSTBM: the terminal
/// ignores the margins and the scroll takes the working line and composer with it.
#[test]
fn a_short_screen_keeps_its_floor_through_a_growth() -> TestResult {
    let mut backend = VT100Backend::with_scrollback(40, 6, 200);
    {
        use std::io::Write;
        backend.write_all(&b"\n".repeat(5))?;
    }
    let mut terminal = yi_tui::terminal::Terminal::new(backend, 4)?;
    let mut app = App::new(
        options(),
        Theme::new(ColorTier::TrueColor, true),
        default_keymap(),
        40,
    );
    app.set_rows(6);
    app.reduce_agent(yi_types::event::AgentEvent::AgentStart);
    yi_tui::render::draw(&mut app, &mut terminal, None);
    app.reduce_agent(streamed("hello there\n"));
    yi_tui::render::draw(&mut app, &mut terminal, None);

    let contents = terminal.backend().contents();
    for expected in ["hello there", "interrupt", "╭", "╰", "faux-1"] {
        assert!(
            contents.contains(expected),
            "a six-row screen lost {expected:?} to the growth:\n{contents}"
        );
    }
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

/// The live region used to render thought only while `live_markdown` was
/// empty, so the first token of prose swapped the reasoning off screen — and
/// the thought had committed nothing, so there was no scrollback to fall back
/// to either. Both halves hold their place now.
#[test]
fn reasoning_stays_on_screen_once_the_prose_starts() -> TestResult {
    let backend = VT100Backend::with_scrollback(80, 24, 200);
    let mut terminal = yi_tui::terminal::Terminal::new(backend, 4)?;
    let mut app = app();
    app.reduce_agent(yi_types::event::AgentEvent::AgentStart);

    // The thought's first paragraph is stable and commits; the second is the
    // tail still streaming, and the prose arrives under both.
    let partial = |thinking: &str, text: &str| {
        let mut content = vec![yi_runtime::faux::faux_thinking(thinking)];
        if !text.is_empty() {
            content.push(yi_runtime::faux::faux_text(text));
        }
        yi_runtime::faux::faux_assistant_message(content, StopReason::Stop)
    };
    let thought = "Weighed the first option.\n\nStill weighing the second";
    for message in [partial(thought, ""), partial(thought, "The answer.")] {
        app.reduce_agent(yi_types::event::AgentEvent::MessageUpdate {
            message: message.clone(),
            assistant_message_event: yi_types::event::AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: String::new(),
                partial: message,
            },
        });
        yi_tui::render::draw(&mut app, &mut terminal, None);
    }

    let contents = terminal.backend().contents();
    let row_of = |needle: &str| {
        contents
            .lines()
            .position(|line| line.contains(needle))
            .ok_or_else(|| format!("{needle:?} is not on screen:\n{contents}"))
    };
    let committed = row_of("Weighed the first option.")?;
    let tail = row_of("Still weighing the second")?;
    let prose = row_of("The answer.")?;
    assert!(
        committed < tail && tail < prose,
        "reasoning keeps its place and its order under the prose:\n{contents}"
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
    app.reduce_agent(streamed(streaming));
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
        app.reduce_agent(streamed(&body));
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

/// A slash command with arguments must reach the dispatcher intact: completing
/// to the highlighted item would silently drop everything after the name, which
/// is the whole payload of `/advisor promote <id>`.
#[test]
fn a_slash_query_with_arguments_runs_verbatim() -> TestResult {
    use yi_tui::keymap::{KeyCodeValue, SingleKey};
    use yi_tui::popup::{BottomView, ListPopup, PopupResult};

    let key = |code: KeyCodeValue| SingleKey {
        code,
        ctrl: false,
        alt: false,
        shift: false,
    };
    let mut popup = ListPopup::new('/', vec!["advisor".to_owned(), "undo".to_owned()]);
    for character in "advisor promote adv-3".chars() {
        popup.handle_key(&key(KeyCodeValue::Char(character)));
    }
    match popup.handle_key(&key(KeyCodeValue::Enter)) {
        PopupResult::Insert(line) => assert_eq!(line, "/advisor promote adv-3"),
        other => return Err(format!("expected the typed line, got {other:?}").into()),
    }

    let mut plain = ListPopup::new('/', vec!["advisor".to_owned(), "undo".to_owned()]);
    for character in "und".chars() {
        plain.handle_key(&key(KeyCodeValue::Char(character)));
    }
    match plain.handle_key(&key(KeyCodeValue::Enter)) {
        PopupResult::Insert(line) => assert_eq!(line, "/undo", "a bare query still completes"),
        other => return Err(format!("expected completion, got {other:?}").into()),
    }

    let mut files = ListPopup::new('@', vec!["src/main.rs".to_owned()]);
    for character in "src ".chars() {
        files.handle_key(&key(KeyCodeValue::Char(character)));
    }
    match files.handle_key(&key(KeyCodeValue::Enter)) {
        PopupResult::Close => {}
        other => {
            return Err(format!("the @ popup must not run text as a command: {other:?}").into());
        }
    }
    Ok(())
}

#[test]
fn a_picked_model_and_effort_reach_the_session_and_the_status_line() -> TestResult {
    let mut app = app();
    let session = Arc::new(faux_session("ok"));
    let fable = yi_runtime::resolve_model("anthropic", "claude-fable-5")
        .ok_or("bundled catalog missing claude-fable-5")?;

    app.selection
        .select(fable.clone(), yi_types::model::Effort::Max);
    yi_tui::commands::process_pending_selection(&mut app, &session);

    assert_eq!(session.model().id, "claude-fable-5");
    assert_eq!(session.effort(), yi_types::model::Effort::Max);
    assert_eq!(app.selection.effort, yi_types::model::Effort::Max);

    let status = yi_tui::status::render(
        &yi_tui::status::StatusInput {
            model: app.selection.model.id.clone(),
            thinking: Some(app.selection.effort.to_string()),
            cwd: "/tmp".to_owned(),
            session_name: "e2e".to_owned(),
            ..Default::default()
        },
        80,
        &Theme::new(ColorTier::TrueColor, true),
    );
    let text: String = status
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert!(text.contains("claude-fable-5"), "{text}");
    assert!(text.contains("max"), "{text}");
    Ok(())
}

/// A level the target model rejects is clamped on the way in, and the session
/// is the authority for what it ended up as.
#[test]
fn the_session_clamp_wins_over_the_requested_level() -> TestResult {
    let mut app = app();
    let session = Arc::new(faux_session("ok"));
    let haiku = yi_runtime::resolve_model("anthropic", "claude-haiku-4-5")
        .ok_or("bundled catalog missing claude-haiku-4-5")?;

    app.selection.select(haiku, yi_types::model::Effort::Max);
    yi_tui::commands::process_pending_selection(&mut app, &session);

    assert_eq!(session.effort(), yi_types::model::Effort::High);
    assert_eq!(app.selection.effort, yi_types::model::Effort::High);
    Ok(())
}

fn priced(text: &str, cost: f64) -> yi_types::message::AgentMessage {
    let mut message = yi_runtime::faux::faux_assistant_message(
        vec![yi_runtime::faux::faux_text(text)],
        StopReason::Stop,
    );
    if let yi_types::message::AgentMessage::Assistant { usage, .. } = &mut message {
        usage.total_tokens = 100;
        usage.cost.total = Number::from_f64(cost).unwrap_or_else(|| Number::from(0u64));
    }
    message
}

/// The HUD's `$` is what the session has spent, not what the last turn cost:
/// a two-turn run showed the second turn's price and called it the total.
#[test]
fn the_status_cost_sums_the_session_not_the_last_turn() -> TestResult {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let dir = std::env::temp_dir().join(format!("yi-tui-cost-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let provider = Arc::new(ProviderStream::new(None, None));
    provider.queue_faux(vec![priced("one", 0.02), priced("two", 0.03)]);
    let session = Arc::new(AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: yi_runtime::ExecutionMode::Sequential,
        },
        provider,
    ));
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        depth: 0,
        max_depth: 1,
        max_children: 4,
        parent_session_dir: dir.clone(),
        cwd: dir.clone(),
        defaults: Arc::new(|| (faux_model(), yi_types::model::Effort::Medium)),
        factory: Arc::new(|_build| Ok(faux_session("child answer"))),
        notice: Arc::new(|_notice| {}),
        events: tokio::sync::broadcast::channel(64).0,
        parent_messages: Arc::new(Vec::new),
        report: Arc::new(|_message| {}),
        attribute: Arc::new(|_usage| {}),
        store: Arc::new(|| None),
    }));
    let (_ask_tx, ask_rx) = std::sync::mpsc::channel();
    let script = yi_tui::parse_script(
        "type ping\nkey enter\nwait-idle 10000\ntype pong\nkey enter\nwait-idle 10000\nquit\n",
    )?;
    let code = yi_tui::run_headless(
        runtime,
        Arc::clone(&session),
        host,
        ask_rx,
        options(),
        yi_tui::DriveOptions {
            script,
            frames_dir: Some(dir.clone()),
            record: None,
            snap: None,
            deadline_secs: 60,
            width: 80,
            height: 24,
        },
    );
    assert_eq!(code, 0, "drive script must run clean");
    let mut frames: Vec<_> = std::fs::read_dir(&dir)?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "txt"))
        .collect();
    frames.sort();
    let last = std::fs::read_to_string(frames.last().ok_or("no frames dumped")?)?;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        last.contains("$0.05"),
        "the status line must carry both turns ($0.02 + $0.03), not the last one:\n{last}"
    );
    Ok(())
}

/// A subagent's spend is the session's spend. Children run unfocused, so the
/// cost has to land without anyone opening the child's transcript.
#[test]
fn an_unfocused_child_turn_reaches_the_status_cost() -> TestResult {
    let backend = VT100Backend::with_scrollback(80, 24, 200);
    let mut terminal = yi_tui::terminal::Terminal::new(backend, 6)?;
    let mut app = app();
    app.reduce_agent(yi_types::event::AgentEvent::MessageEnd {
        message: priced("parent", 0.02),
    });
    app.reduce_child(
        "child-1",
        yi_types::event::AgentEvent::MessageEnd {
            message: priced("child", 0.04),
        },
    );
    yi_tui::render::draw(&mut app, &mut terminal, None);
    let contents = terminal.backend().contents();
    assert!(
        contents.contains("$0.06"),
        "the child's spend belongs to the session total: {contents}"
    );
    Ok(())
}

/// Strip `TestBackend`'s per-row quoting and the trailing blanks a terminal
/// screen and a text dump disagree about, so the two can be compared at all.
fn screen_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| line.trim_matches('"').trim_end().to_owned())
        .collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// The recording and the frames the script asserted on are two sinks for one
/// draw stream, so they must not disagree: replaying the cast through a real
/// terminal parser has to land on the frame the run finished with. This is
/// what makes a GIF handed to a reviewer evidence of the same UI the
/// assertions passed against.
#[test]
fn a_recording_replays_to_the_frame_the_run_asserted_on() -> TestResult {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let dir = std::env::temp_dir().join(format!("yi-tui-cast-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let session = Arc::new(faux_session("recorded reply"));
    let host = Arc::new(SubagentHost::new(SubagentHostOptions {
        depth: 0,
        max_depth: 1,
        max_children: 4,
        parent_session_dir: dir.clone(),
        cwd: dir.clone(),
        defaults: Arc::new(|| (faux_model(), yi_types::model::Effort::Medium)),
        factory: Arc::new(|_build| Ok(faux_session("child answer"))),
        notice: Arc::new(|_notice| {}),
        events: tokio::sync::broadcast::channel(64).0,
        parent_messages: Arc::new(Vec::new),
        report: Arc::new(|_message| {}),
        attribute: Arc::new(|_usage| {}),
        store: Arc::new(|| None),
    }));
    let (_ask_tx, ask_rx) = std::sync::mpsc::channel();
    let cast = dir.join("run.cast");
    let still = dir.join("still.cast");
    let script = yi_tui::parse_script("type ping\nkey enter\nwait-idle 10000\nquit\n")?;
    let code = yi_tui::run_headless(
        runtime,
        Arc::clone(&session),
        host,
        ask_rx,
        options(),
        yi_tui::DriveOptions {
            script,
            frames_dir: Some(dir.clone()),
            record: Some(cast.clone()),
            snap: Some(still.clone()),
            deadline_secs: 60,
            width: 80,
            height: 24,
        },
    );
    assert_eq!(code, 0, "drive script must run clean");

    let recording = std::fs::read_to_string(&cast)?;
    let mut lines = recording.lines();
    let header: serde_json::Value = serde_json::from_str(lines.next().ok_or("empty cast")?)?;
    assert_eq!(header["version"], 2, "asciicast v2 header: {header}");
    assert_eq!(header["width"], 80);
    assert_eq!(header["height"], 24);

    let mut parser = vt100::Parser::new(24, 80, 0);
    let mut previous = 0.0_f64;
    let mut events = 0_usize;
    for line in lines {
        let event: serde_json::Value = serde_json::from_str(line)?;
        let time = event[0].as_f64().ok_or("event time")?;
        assert!(
            time >= previous,
            "cast time went backwards: {time} < {previous}"
        );
        previous = time;
        assert_eq!(event[1], "o", "only output events: {event}");
        parser.process(event[2].as_str().ok_or("event payload")?.as_bytes());
        events += 1;
    }

    let mut frames: Vec<_> = std::fs::read_dir(&dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "txt"))
        .collect();
    frames.sort();
    let dumped = std::fs::read_to_string(frames.last().ok_or("no frames dumped")?)?;
    let replayed = parser.screen().contents();
    // The still is a cast of its own so one renderer draws both artifacts.
    // It has to stand alone: a single event that paints the whole screen.
    let still_text = std::fs::read_to_string(&still)?;
    let mut still_lines = still_text.lines();
    let still_header: serde_json::Value =
        serde_json::from_str(still_lines.next().ok_or("empty still")?)?;
    assert_eq!(still_header["version"], 2, "{still_header}");
    let still_events: Vec<&str> = still_lines.collect();
    assert_eq!(
        still_events.len(),
        1,
        "one frame, one event: {still_events:?}"
    );
    let still_event: serde_json::Value = serde_json::from_str(still_events[0])?;
    let mut still_parser = vt100::Parser::new(24, 80, 0);
    still_parser.process(still_event[2].as_str().ok_or("still payload")?.as_bytes());
    let still_screen = still_parser.screen().contents();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(
        screen_lines(&replayed),
        screen_lines(&dumped),
        "the cast must replay to the frame the assertions ran against"
    );
    // The loop draws every couple of milliseconds; only changed frames may
    // reach the cast, or a minute of idling buries the recording in no-ops.
    assert!(
        events <= frames.len() * 4 + 20,
        "{events} events for {} changed frames — the no-op filter regressed",
        frames.len()
    );
    // The point of the still: painted cold into an empty terminal, it must
    // land on the same screen the recording ends on.
    assert_eq!(
        screen_lines(&still_screen),
        screen_lines(&dumped),
        "the still must paint the frame the run ended on"
    );
    Ok(())
}
