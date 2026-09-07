#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

//! `yi console` — the multi-agent workspace shell: an ACP client over the
//! `yi serve` daemon socket; the daemon owns every session, this renders.

pub mod app;
pub mod avatar;
pub mod client;
pub mod keys;
pub mod kitty;
pub mod layout;
pub mod model;
pub mod notify;
pub mod palette;
pub mod render;
pub mod select;
pub mod sidebar;

use std::io::Stdout;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{
    self as ct_event, DisableBracketedPaste, EnableBracketedPaste, Event as CtEvent,
};
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::crossterm::{ExecutableCommand, execute};
use yi_tui::capture::RecordingBackend;
use yi_tui::colors::{ColorTier, Theme, detect_dark, detect_tier};
use yi_tui::drive::{Step, WaitPoll, key_event, poll_condition, typed_events};

use crate::app::{App, MouseKind};
use crate::client::{ClientEvent, Outbound};
use crate::model::{Link, SessionStatus};

pub struct ConsoleOptions {
    pub socket: PathBuf,
    pub root: String,
    pub autostart: bool,
    pub auto_side: bool,
    pub sidebar: crate::model::SidebarMode,
}

pub struct DriveOptions {
    pub script: Vec<ConsoleStep>,
    pub frames_dir: Option<PathBuf>,
    pub record: Option<PathBuf>,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone)]
pub enum ConsoleStep {
    Tui(Step),
    Mouse(MouseKind, u16, u16),
    Cmd(yi_tui::keymap::SingleKey),
}

pub fn parse_script(source: &str) -> Result<Vec<ConsoleStep>, String> {
    let mut steps = Vec::new();
    for (index, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let error = |message: String| format!("line {}: {message}", index.saturating_add(1));
        if let Some(rest) = line.strip_prefix("mouse ") {
            let mut parts = rest.split_whitespace();
            let kind = match parts.next() {
                Some("down") => MouseKind::Down,
                Some("up") => MouseKind::Up,
                Some("drag") => MouseKind::Drag,
                Some("scrollup") => MouseKind::ScrollUp,
                Some("scrolldown") => MouseKind::ScrollDown,
                other => return Err(error(format!("unknown mouse kind: {other:?}"))),
            };
            let x = parts
                .next()
                .and_then(|value| value.parse::<u16>().ok())
                .ok_or_else(|| error("mouse needs <kind> <x> <y>".to_owned()))?;
            let y = parts
                .next()
                .and_then(|value| value.parse::<u16>().ok())
                .ok_or_else(|| error("mouse needs <kind> <x> <y>".to_owned()))?;
            steps.push(ConsoleStep::Mouse(kind, x, y));
            continue;
        }
        if let Some(rest) = line.strip_prefix("cmd-") {
            let parsed = yi_tui::drive::parse_script(&format!("key {rest}")).map_err(error)?;
            let Some(Step::Key(key)) = parsed.into_iter().next() else {
                return Err(error(format!("cmd needs a key, got {rest:?}")));
            };
            steps.push(ConsoleStep::Cmd(key));
            continue;
        }
        let parsed = yi_tui::drive::parse_script(line).map_err(error)?;
        steps.extend(parsed.into_iter().map(ConsoleStep::Tui));
    }
    Ok(steps)
}

const FRAME_INTERVAL: Duration = Duration::from_millis(16);

fn drain(app: &mut App, outbound: &Outbound, events: &std::sync::mpsc::Receiver<ClientEvent>) {
    while let Ok(event) = events.try_recv() {
        app.reduce_client(outbound, event);
    }
    app.tick(outbound);
}

fn draw<B: Backend>(
    app: &mut App,
    terminal: &mut Terminal<B>,
    theme: &Theme,
) -> std::io::Result<()> {
    if !app.dirty {
        return Ok(());
    }
    app.dirty = false;
    terminal.draw(|frame| {
        let mut view = render::compute_view(app, frame.area(), theme);
        render::render(app, frame, &mut view, theme);
        let area = frame.area();
        app.selected = match app.selection {
            Some(selection) => {
                crate::select::paint(frame.buffer_mut(), area, selection, theme.selection_bg())
            }
            None => String::new(),
        };
        if let Some(cursor) = view.editor_cursor {
            frame.set_cursor_position(cursor);
        }
    })?;
    Ok(())
}

pub fn run_console(options: &ConsoleOptions) -> i32 {
    let theme = Theme::new(
        detect_tier(
            std::env::var("COLORTERM").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
        ),
        detect_dark(std::env::var("COLORFGBG").ok().as_deref()),
    );
    let (events, outbound, threads) = client::spawn(options.socket.clone());
    let mut app = App::new(options.root.clone(), theme);
    app.autostart = options.autostart;
    app.kitty = crate::kitty::supported(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
    );
    app.state.auto_side = options.auto_side;
    app.state.sidebar = options.sidebar;
    app.animate = true;
    app.osc_flavor = crate::notify::detect_flavor(
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var("KITTY_WINDOW_ID").ok().as_deref(),
    );
    app.cmd_hints = crate::kitty::supported(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
    );

    if let Err(error) = enable_raw_mode() {
        eprintln!("error: raw mode: {error}");
        return 1;
    }
    let mut stdout = std::io::stdout();
    let _ = execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        ratatui::crossterm::event::EnableMouseCapture
    );
    let _ = execute!(
        stdout,
        ratatui::crossterm::event::PushKeyboardEnhancementFlags(
            ratatui::crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | ratatui::crossterm::event::KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        )
    );
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            restore_terminal();
            eprintln!("error: terminal: {error}");
            return 1;
        }
    };

    let code = run_interactive(&mut app, &mut terminal, &events, &outbound, &theme);
    restore_terminal();
    outbound.shutdown();
    drop(events);
    join_with_deadline(threads);
    code
}

fn run_interactive(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    events: &std::sync::mpsc::Receiver<ClientEvent>,
    outbound: &Outbound,
    theme: &Theme,
) -> i32 {
    let kitty_ok = app.kitty;
    // (payload length, rect) of the placed image; unchanged frames skip the
    // retransmit entirely.
    let mut placed: Option<(usize, ratatui::layout::Rect)> = None;
    let mut last_draw = Instant::now()
        .checked_sub(FRAME_INTERVAL)
        .unwrap_or_else(Instant::now);
    while !app.state.quit {
        drain(app, outbound, events);
        match ct_event::poll(Duration::from_millis(50)) {
            Ok(true) => {
                let Ok(event) = ct_event::read() else {
                    return 1;
                };
                app.handle_event(outbound, event);
                while let Ok(true) = ct_event::poll(Duration::ZERO) {
                    let Ok(event) = ct_event::read() else { break };
                    app.handle_event(outbound, event);
                }
            }
            Ok(false) => {}
            Err(error) => {
                eprintln!("error: input: {error}");
                return 1;
            }
        }
        if app.dirty && last_draw.elapsed() >= FRAME_INTERVAL {
            last_draw = Instant::now();
            // The whole frame lands inside one synchronized update, so a
            // split-ratio animation step can never tear.
            let mut out = std::io::stdout();
            let _ = out.execute(ratatui::crossterm::terminal::BeginSynchronizedUpdate);
            let drawn = draw(app, terminal, theme);
            let _ = out.execute(ratatui::crossterm::terminal::EndSynchronizedUpdate);
            if drawn.is_err() {
                return 1;
            }
            set_window_title(app);
            for sequence in app.osc_out.drain(..) {
                use std::io::Write;
                let _ = out.write_all(sequence.as_bytes());
                let _ = out.flush();
            }
            if kitty_ok {
                use std::io::Write;
                place_chat_orbs(app, &mut out);
                place_avatars(app, &mut out);
                place_notebook_image(app, &mut out, &mut placed);
                let _ = out.flush();
            }
        }
    }
    0
}

fn set_window_title(app: &mut App) {
    let title = app
        .state
        .focused_session()
        .and_then(|id| app.state.sessions.get(&id))
        .map_or_else(|| "yi".to_owned(), |row| format!("yi · {}", row.label()));
    if title != app.window_title {
        app.osc_out.push(format!("\x1b]2;{title}\x07"));
        app.window_title = title;
    }
}

/// The identicon over every rail row on screen; rows that scrolled off are deleted.
fn place_avatars(app: &mut App, out: &mut std::io::Stdout) {
    let rows: Vec<crate::avatar::Placement> = app
        .hits
        .as_ref()
        .map(|hits| hits.avatars.clone())
        .unwrap_or_default();
    app.avatars.sync(out, &rows);
}

/// One orb per chat pane, each on its own image ids; only the focused pane animates.
fn place_chat_orbs(app: &mut App, out: &mut std::io::Stdout) {
    use crate::model::PaneContent;
    let focused = app.state.focused_pane_id();
    for (id, pane) in &mut app.state.panes {
        let PaneContent::Session {
            chat: Some(chat), ..
        } = &mut pane.content
        else {
            continue;
        };
        if Some(*id) == focused {
            yi_tui::orb::tick(&mut chat.app, out, &mut chat.orb);
            yi_tui::logos::tick(&chat.app, out, &mut chat.logos);
        } else {
            chat.orb.hide(out);
            yi_tui::logos::delete_all(out, &mut chat.logos);
        }
    }
}

/// The focused notebook pane's newest image rides the kitty protocol over
/// the top half of the pane; text placeholders stay for every other terminal.
fn place_notebook_image(
    app: &App,
    out: &mut std::io::Stdout,
    placed: &mut Option<(usize, ratatui::layout::Rect)>,
) {
    use crate::model::PaneContent;
    let clear = |out: &mut std::io::Stdout, placed: &mut Option<(usize, ratatui::layout::Rect)>| {
        if placed.take().is_some() {
            let _ = crate::kitty::delete(out);
        }
    };
    let Some(pane_id) = app.state.focused_pane_id() else {
        return clear(out, placed);
    };
    let Some(pane) = app.state.panes.get(&pane_id) else {
        return clear(out, placed);
    };
    let PaneContent::Notebook { cells, .. } = &pane.content else {
        return clear(out, placed);
    };
    let image = cells
        .iter()
        .rev()
        .flat_map(|cell| cell.images.iter())
        .next();
    let Some(image) = image else {
        return clear(out, placed);
    };
    let rect = app
        .hits
        .as_ref()
        .and_then(|hits| hits.panes.iter().find(|(id, _)| *id == pane_id))
        .map(|(_, rect)| *rect);
    let Some(rect) = rect else {
        return clear(out, placed);
    };
    let inner = rect.inner(ratatui::layout::Margin::new(1, 1));
    let half = ratatui::layout::Rect {
        height: inner.height / 2,
        ..inner
    };
    if half.height < 3 || half.width < 8 {
        return clear(out, placed);
    }
    if *placed == Some((image.len(), half)) {
        return;
    }
    if crate::kitty::place_png(out, image, half).is_ok() {
        *placed = Some((image.len(), half));
    }
}

fn restore_terminal() {
    let mut stdout = std::io::stdout();
    let _ = stdout.execute(ratatui::crossterm::event::PopKeyboardEnhancementFlags);
    let _ = stdout.execute(ratatui::crossterm::event::DisableMouseCapture);
    let _ = stdout.execute(DisableBracketedPaste);
    let _ = stdout.execute(LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

fn join_with_deadline(threads: client::ClientThreads) {
    let deadline = Instant::now() + Duration::from_secs(2);
    for handle in [threads.reader, threads.writer] {
        if Instant::now() < deadline {
            let _ = handle.join();
        }
    }
}

/// The same loop over an in-memory screen, driven by a script (the
/// .ruler/085-tui.md drive contract for this crate).
pub fn run_headless(options: &ConsoleOptions, drive: DriveOptions) -> i32 {
    let theme = Theme::new(ColorTier::Ansi16, true);
    let (events, outbound, threads) = client::spawn(options.socket.clone());
    let mut app = App::new(options.root.clone(), theme);
    app.autostart = options.autostart;
    app.state.auto_side = options.auto_side;
    app.state.sidebar = options.sidebar;
    let width = drive.width.max(20);
    let height = drive.height.max(8);
    let backend = match RecordingBackend::new(width, height, drive.record.as_deref()) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("error: headless backend: {error}");
            return 1;
        }
    };
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            eprintln!("error: headless terminal: {error}");
            return 1;
        }
    };
    if let Some(dir) = &drive.frames_dir
        && let Err(error) = std::fs::create_dir_all(dir)
    {
        eprintln!("error: frames dir: {error}");
        return 1;
    }

    let mut frame_index = 0_u32;
    let mut last_frame = String::new();
    let mut steps = drive.script.into_iter();
    let mut current: Option<(ConsoleStep, Instant)> = None;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut exit_code = 0;
    let mut type_ms = 0;

    while !app.state.quit {
        if Instant::now() > deadline {
            eprintln!("error: drive script timed out (60 s wall)");
            exit_code = 1;
            break;
        }
        drain(&mut app, &outbound, &events);

        let step = match current.take() {
            Some(pending) => pending,
            None => match steps.next() {
                Some(step) => (step, Instant::now()),
                None => break,
            },
        };
        match step {
            (ConsoleStep::Mouse(kind, x, y), _) => {
                app.handle_mouse(&outbound, kind, x, y);
            }
            (ConsoleStep::Tui(Step::Key(key)), _) => {
                app.handle_event(&outbound, CtEvent::Key(key_event(&key)));
            }
            (ConsoleStep::Cmd(key), _) => {
                let mut event = key_event(&key);
                event.modifiers |= ratatui::crossterm::event::KeyModifiers::SUPER;
                app.handle_event(&outbound, CtEvent::Key(event));
            }
            (ConsoleStep::Tui(Step::TypeMs(ms)), _) => type_ms = ms,
            (ConsoleStep::Tui(Step::Type(text)), _) => {
                for event in typed_events(&text) {
                    app.handle_event(&outbound, event);
                    if type_ms > 0 {
                        std::thread::sleep(Duration::from_millis(type_ms));
                    }
                }
            }
            (ConsoleStep::Tui(Step::Wait(ms)), started) => {
                if started.elapsed() < Duration::from_millis(ms) {
                    current = Some((ConsoleStep::Tui(Step::Wait(ms)), started));
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            (ConsoleStep::Tui(Step::WaitIdle(ms)), started) => {
                let working = app
                    .state
                    .focused_session()
                    .as_ref()
                    .and_then(|id| app.state.sessions.get(id))
                    .is_some_and(|row| row.status == SessionStatus::Working)
                    || app.chat_running()
                    || app.state.link == Link::Connecting;
                match poll_condition(!working, started, ms, "wait-idle") {
                    WaitPoll::Retry => {
                        current = Some((ConsoleStep::Tui(Step::WaitIdle(ms)), started))
                    }
                    WaitPoll::TimedOut => exit_code = 1,
                    WaitPoll::Done => {}
                }
            }
            (ConsoleStep::Tui(Step::WaitFrame(ms, present, text)), started) => {
                let shown = terminal.backend().screen().contains(&text) == present;
                match poll_condition(shown, started, ms, &format!("wait-frame {text:?}")) {
                    WaitPoll::Done => {}
                    WaitPoll::Retry => {
                        current = Some((
                            ConsoleStep::Tui(Step::WaitFrame(ms, present, text)),
                            started,
                        ))
                    }
                    WaitPoll::TimedOut => exit_code = 1,
                }
            }
            (ConsoleStep::Tui(Step::Quit), _) => break,
        }

        app.dirty = true;
        if draw(&mut app, &mut terminal, &theme).is_err() {
            exit_code = 1;
            break;
        }
        if let Some(dir) = &drive.frames_dir {
            let frame = terminal.backend().screen();
            if frame != last_frame {
                let path = dir.join(format!("{frame_index:04}.txt"));
                if std::fs::write(&path, &frame).is_ok() {
                    frame_index = frame_index.saturating_add(1);
                    last_frame = frame;
                }
            }
        }
    }

    outbound.shutdown();
    drop(events);
    join_with_deadline(threads);
    exit_code
}
