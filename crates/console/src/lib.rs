#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

//! `yi console` — the multi-agent workspace shell: an ACP client over the
//! `yi serve` daemon socket; the daemon owns every session, this renders.

pub mod app;
pub mod avatar;
mod chrome;
pub mod client;
pub mod keys;
pub mod kitty;
pub mod layout;
pub mod menu;
pub mod model;
pub mod notify;
pub mod palette;
pub mod panes;
pub mod render;
pub mod select;
pub mod settings;
pub mod sidebar;
mod title;

use std::io::Stdout;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{DisableBracketedPaste, EnableBracketedPaste, Event as CtEvent};
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::crossterm::{ExecutableCommand, execute};
use yi_tui::capture::RecordingBackend;
use yi_tui::colors::{ColorTier, Theme, detect_dark, detect_tier};
use yi_tui::drive::{
    Control, Deadline, Step, WaitPoll, keep_frame, key_event, poll_condition, typed_events,
};
use yi_tui::term::ControlPictures;

use crate::app::{App, MouseKind};
use crate::client::{ClientEvent, Outbound};
use crate::layout::PaneId;
use crate::model::{Link, SessionId, SessionStatus};

pub use title::{TitleOptions, UnknownScene};
use title::{title_playing, title_tick};

pub struct ConsoleOptions {
    pub socket: PathBuf,
    pub root: String,
    pub autostart: bool,
    pub auto_side: bool,
    pub title: Option<TitleOptions>,
    pub open_apps: Vec<String>,
    pub sidebar: crate::model::SidebarMode,
    pub pace: u16,
}

pub struct DriveOptions {
    pub script: Vec<ConsoleStep>,
    pub frames_dir: Option<PathBuf>,
    pub record: Option<PathBuf>,
    /// Wall clock the whole script gets; a paced console scene outlives the 60 s an assertion
    /// run needs.
    pub deadline_secs: u64,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone)]
pub enum ConsoleStep {
    Tui(Step),
    Mouse(MouseKind, u16, u16),
    Cmd(yi_tui::keymap::SingleKey),
}

/// The drive harness speaks crossterm, so a script runs the same button mapping a terminal does.
fn mouse_event(kind: MouseKind, column: u16, row: u16) -> ratatui::crossterm::event::MouseEvent {
    use ratatui::crossterm::event::{MouseButton, MouseEventKind};
    let kind = match kind {
        MouseKind::Down => MouseEventKind::Down(MouseButton::Left),
        MouseKind::RightDown => MouseEventKind::Down(MouseButton::Right),
        MouseKind::Moved => MouseEventKind::Moved,
        MouseKind::Up => MouseEventKind::Up(MouseButton::Left),
        MouseKind::Drag => MouseEventKind::Drag(MouseButton::Left),
        MouseKind::ScrollUp => MouseEventKind::ScrollUp,
        MouseKind::ScrollDown => MouseEventKind::ScrollDown,
    };
    ratatui::crossterm::event::MouseEvent {
        kind,
        column,
        row,
        modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
    }
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
                Some("right") => MouseKind::RightDown,
                Some("move") => MouseKind::Moved,
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
        if parsed
            .iter()
            .any(|step| matches!(step, Step::Hold(..) | Step::Release))
        {
            return Err(error(
                "hold and release need the solo drive: the console's model lives in the daemon"
                    .to_owned(),
            ));
        }
        steps.extend(parsed.into_iter().map(ConsoleStep::Tui));
    }
    yi_tui::drive::refuse_repeated_snaps(steps.iter().filter_map(|step| match step {
        ConsoleStep::Tui(step) => Some(step),
        ConsoleStep::Mouse(..) | ConsoleStep::Cmd(_) => None,
    }))?;
    Ok(steps)
}

fn timing_step(
    app: &mut App,
    outbound: &Outbound,
    events: &std::sync::mpsc::Receiver<ClientEvent>,
    step: &Step,
    started: Instant,
) -> Option<WaitPoll> {
    Some(match step {
        Step::Tick(ms) => {
            if app.clock == yi_tui::motion::Clock::Real {
                app.clock = yi_tui::motion::Clock::Fixed(Instant::now());
            }
            tick(app, outbound, events, *ms);
            WaitPoll::Done
        }
        Step::WaitIdle(ms) => poll_condition(!drive_idle(app), started, *ms, "wait-idle"),
        Step::Hold(..) | Step::Release => WaitPoll::Failed(
            "hold and release need the solo drive: the console's model lives in the daemon"
                .to_owned(),
        ),
        _ => return None,
    })
}

/// Moves the console's frozen clock a frame at a time, its chat panes with it.
fn tick(
    app: &mut App,
    outbound: &Outbound,
    events: &std::sync::mpsc::Receiver<ClientEvent>,
    ms: u64,
) {
    let mut left = ms;
    while left > 0 {
        let step = left.min(yi_tui::drive::TICK_STEP_MS);
        app.clock.advance(Duration::from_millis(step));
        drain(app, outbound, events);
        left -= step;
    }
}

/// The idle tick: request deadlines, the list poll and animations advance at least this often.
const IDLE_POLL: Duration = Duration::from_millis(50);

fn drain(app: &mut App, outbound: &Outbound, events: &std::sync::mpsc::Receiver<ClientEvent>) {
    let mut span = yi_types::trace::span("console.drain");
    let mut count = 0_u64;
    while let Ok(event) = events.try_recv() {
        count = count.saturating_add(1);
        app.reduce_client(outbound, event);
    }
    span.set("events", count);
    let _tick = yi_types::trace::span("console.tick");
    app.tick(outbound);
    title_tick(app);
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
    if std::mem::take(&mut app.pending_clear) {
        terminal.clear()?;
        app.notebook_image = None;
    }
    let _span = yi_types::trace::span("console.draw");
    terminal.draw(|frame| {
        let computing = yi_types::trace::span("console.compute_view");
        let mut view = render::compute_view(app, frame.area(), theme);
        drop(computing);
        let _rendering = yi_types::trace::span("console.render");
        let scroll = |app: &App| {
            let drag = app.selection.as_ref()?;
            Some(app.state.panes.get(&drag.pane)?.scroll_from_bottom)
        };
        let before = scroll(app);
        render::render(app, frame, &mut view, theme);
        app.selected.clear();
        if let Some(mut drag) = app.selection.take() {
            // A held view that grew below raises its scroll with nothing moving on screen.
            if let (Some(before), Some(after)) = (before, scroll(app))
                && after > before
            {
                drag.shift(after - before);
            }
            app.selected = crate::select::selected(
                app,
                &view,
                frame.buffer_mut(),
                &mut drag,
                theme.selection_bg(),
            );
            app.selection = Some(drag);
        }
        if let Some(cursor) = view.editor_cursor {
            frame.set_cursor_position(cursor);
        }
    })?;
    Ok(())
}

pub fn run_console(options: &ConsoleOptions, control: Option<Control>) -> i32 {
    let theme = Theme::new(
        detect_tier(
            std::env::var("COLORTERM").ok().as_deref(),
            std::env::var("TERM").ok().as_deref(),
        ),
        detect_dark(std::env::var("COLORFGBG").ok().as_deref()),
    );
    let setup = yi_types::trace::span("console.setup");
    yi_tui::highlight::prewarm();
    let (event_tx, events) = client::events();
    let (outbound, threads) = client::spawn(options.socket.clone(), event_tx.clone());
    let mut app = App::new(options.root.clone(), theme);
    app.kitty = crate::kitty::supported(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
    );
    configure(&mut app, options);
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
    let backend = ControlPictures(CrosstermBackend::new(stdout));
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            restore_terminal();
            eprintln!("error: terminal: {error}");
            return 1;
        }
    };

    let stop_input = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let input = client::spawn_input(event_tx, std::sync::Arc::clone(&stop_input));
    drop(setup);
    let code = run_interactive(&mut app, &mut terminal, &events, &outbound, &theme, control);
    stop_input.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = input.join();
    restore_terminal();
    outbound.shutdown();
    drop(events);
    join_with_deadline(threads);
    code
}

fn run_interactive(
    app: &mut App,
    terminal: &mut Terminal<ControlPictures<CrosstermBackend<Stdout>>>,
    events: &std::sync::mpsc::Receiver<ClientEvent>,
    outbound: &Outbound,
    theme: &Theme,
    mut control: Option<Control>,
) -> i32 {
    let mut orb_owners = std::collections::HashMap::new();
    let mut scheduler = yi_tui::frame::FrameScheduler::default();
    while !app.state.quit {
        drain(app, outbound, events);
        if app.dirty {
            scheduler.request();
        }
        let wake = scheduler
            .poll_timeout(Instant::now())
            .min(IDLE_POLL)
            .min(chat_wake(app))
            .min(app.avatars.wake().unwrap_or(IDLE_POLL))
            .min(
                control
                    .as_ref()
                    .map_or(IDLE_POLL, |_| yi_tui::drive::CONTROL_POLL),
            );
        // Incident: a 50 ms input poll delayed every daemon frame; any event now wakes the loop.
        match events.recv_timeout(wake) {
            Ok(ClientEvent::InputClosed(error)) => {
                eprintln!("error: input: {error}");
                return 1;
            }
            Ok(event) => app.reduce_client(outbound, event),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return 1,
        }
        drain(app, outbound, events);
        if yi_tui::drive::serve(&mut control, |step, at| {
            timing_step(app, outbound, events, step, at)
        }) {
            app.dirty = true;
        }
        if app.dirty {
            scheduler.request();
        }
        let mut out = std::io::stdout();
        if step_frame(
            app,
            terminal,
            theme,
            &mut scheduler,
            &mut orb_owners,
            &mut out,
        )
        .is_err()
        {
            return 1;
        }
    }
    0
}

/// Incident: with no text to draw the orb and avatars stopped advancing, so an orb froze
/// mid-morph once a turn ended; that step still moves them, in an update of their own.
fn step_frame<B: Backend>(
    app: &mut App,
    terminal: &mut Terminal<B>,
    theme: &Theme,
    scheduler: &mut yi_tui::frame::FrameScheduler,
    orb_owners: &mut std::collections::HashMap<PaneId, Option<SessionId>>,
    out: &mut impl std::io::Write,
) -> std::io::Result<()> {
    if scheduler.should_draw(Instant::now()) {
        let start = Instant::now();
        present(app, terminal, theme, orb_owners, out)?;
        set_window_title(app);
        for sequence in app.osc_out.drain(..) {
            let _ = out.write_all(sequence.as_bytes());
            let _ = out.flush();
        }
        scheduler.mark_drawn(start, Instant::now());
    } else if app.kitty {
        let _ = yi_tui::term::framed(out, |out| {
            place_chat_orbs(app, orb_owners, out);
            app.avatars.animate(out);
        });
    }
    Ok(())
}

/// Incident: images placed after the update closed sat on their old rows over moved text, so
/// the text and every image the frame moves land in one synchronized update.
fn present<B: Backend>(
    app: &mut App,
    terminal: &mut Terminal<B>,
    theme: &Theme,
    orb_owners: &mut std::collections::HashMap<PaneId, Option<SessionId>>,
    out: &mut impl std::io::Write,
) -> std::io::Result<()> {
    let _ = out.execute(ratatui::crossterm::terminal::BeginSynchronizedUpdate);
    let drawn = draw(app, terminal, theme);
    if app.kitty {
        place_avatars(app, out);
        place_notebook_image(app, out);
        place_chat_orbs(app, orb_owners, out);
        app.avatars.animate(out);
    }
    let _ = out.execute(ratatui::crossterm::terminal::EndSynchronizedUpdate);
    drawn
}

fn set_window_title(app: &mut App) {
    let title = app
        .state
        .focused_session()
        .and_then(|id| app.state.sessions.get(&id))
        .map_or_else(|| "yi".to_owned(), |row| format!("yi · {}", row.label()));
    if title != app.window_title {
        app.osc_out.push(yi_tui::term::window_title_osc(&title));
        app.window_title = title;
    }
}

/// The identicon over every rail row on screen; rows that scrolled off are deleted.
fn place_avatars(app: &mut App, out: &mut impl std::io::Write) {
    let rows: Vec<crate::avatar::Placement> = app
        .hits
        .as_ref()
        .filter(|_| !title_playing(app))
        .map(|hits| crate::avatar::uncovered(hits.avatars.clone(), &hits.overlays))
        .unwrap_or_default();
    app.avatars.sync(out, &rows);
}

/// Incident: the reveal's clock was missing here, so a streaming chat stepped only when the orb
/// or the idle poll woke the loop and drew at 28 fps in 2-row lurches.
fn chat_wake(app: &App) -> Duration {
    use crate::model::PaneContent;
    app.state
        .panes
        .values()
        .filter_map(|pane| match &pane.content {
            PaneContent::Session {
                chat: Some(chat), ..
            } => {
                let orb = chat.app.orb_animating().then(|| chat.orb.wake());
                Some(chat.app.reveal_wake().min(orb.unwrap_or(Duration::MAX)))
            }
            _ => None,
        })
        .min()
        .unwrap_or(IDLE_POLL)
        .min(if title_playing(app) {
            yi_tui::title::FRAME
        } else {
            Duration::MAX
        })
}

/// One orb per chat pane on its own image ids; only the focused pane animates. Incident: a
/// closed or switched pane kept its orb, as the next chat's `Tick` deletes only what it placed.
fn place_chat_orbs(
    app: &mut App,
    owners: &mut std::collections::HashMap<PaneId, Option<SessionId>>,
    out: &mut impl std::io::Write,
) {
    use crate::model::PaneContent;
    owners.retain(|pane, owner| {
        let kept = app
            .state
            .panes
            .get(pane)
            .is_some_and(|pane| match &pane.content {
                PaneContent::Session {
                    session,
                    chat: Some(_),
                } => session == owner,
                _ => false,
            });
        if !kept {
            for id in crate::app::orb_ids(*pane) {
                let _ = yi_tui::orb::kitty::delete_id(out, id);
            }
        }
        kept
    });
    let focused = app.state.focused_pane_id();
    let covered = |id: PaneId| {
        app.hits.as_ref().is_some_and(|hits| {
            let pane = hits.panes.iter().find(|(pane, _)| *pane == id);
            pane.is_some_and(|(_, rect)| hits.overlays.iter().any(|cover| cover.intersects(*rect)))
        })
    };
    let shown = focused.filter(|id| !covered(*id));
    for (id, pane) in &mut app.state.panes {
        let PaneContent::Session {
            session,
            chat: Some(chat),
        } = &mut pane.content
        else {
            continue;
        };
        owners.entry(*id).or_insert_with(|| session.clone());
        if Some(*id) == shown {
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
fn place_notebook_image(app: &mut App, out: &mut impl std::io::Write) {
    use crate::model::PaneContent;
    let playing = title_playing(app);
    let placed = &mut app.notebook_image;
    let clear = |out: &mut _, placed: &mut Option<(usize, ratatui::layout::Rect)>| {
        if placed.take().is_some() {
            let _ = crate::kitty::delete(out);
        }
    };
    let Some(pane_id) = app.state.focused_pane_id().filter(|_| !playing) else {
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
    let covered = app
        .hits
        .as_ref()
        .is_some_and(|hits| hits.overlays.iter().any(|cover| cover.intersects(half)));
    if half.height < 3 || half.width < 8 || covered {
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

fn drive_idle(app: &App) -> bool {
    app.state
        .focused_session()
        .as_ref()
        .and_then(|id| app.state.sessions.get(id))
        .is_some_and(|row| row.status == SessionStatus::Working)
        || app.chat_running()
        || app.state.link == Link::Connecting
}

/// The same loop over an in-memory screen, driven by a script (the
/// .ruler/085-tui.md drive contract for this crate).
pub fn run_headless(options: &ConsoleOptions, drive: DriveOptions) -> i32 {
    let theme = Theme::new(ColorTier::Ansi16, true);
    let (event_tx, events) = client::events();
    let (outbound, threads) = client::spawn(options.socket.clone(), event_tx);
    let mut app = App::new(options.root.clone(), theme);
    if drive
        .script
        .iter()
        .any(|step| matches!(step, ConsoleStep::Tui(Step::Tick(_))))
    {
        app.clock = yi_tui::motion::Clock::Fixed(Instant::now());
    }
    configure(&mut app, options);
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
    let deadline = Deadline::new(drive.deadline_secs);
    let mut exit_code = 0;
    let mut type_ms = 0;
    let mut pending_snap = None;

    while !app.state.quit {
        if deadline.passed() {
            deadline.report();
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
                app.handle_event(&outbound, CtEvent::Mouse(mouse_event(kind, x, y)));
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
            (ConsoleStep::Tui(Step::Snap(name)), _) => pending_snap = Some(name),
            (ConsoleStep::Tui(Step::Tick(ms)), _) => tick(&mut app, &outbound, &events, ms),
            (ConsoleStep::Tui(Step::Hold(..) | Step::Release), _) => {}
            (ConsoleStep::Tui(Step::Type(text)), _) => {
                for event in typed_events(&text) {
                    app.handle_event(&outbound, event);
                    if type_ms > 0 {
                        std::thread::sleep(Duration::from_millis(type_ms));
                        if deadline.passed() {
                            break;
                        }
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
                match poll_condition(!drive_idle(&app), started, ms, "wait-idle") {
                    WaitPoll::Retry => {
                        current = Some((ConsoleStep::Tui(Step::WaitIdle(ms)), started))
                    }
                    WaitPoll::Failed(error) => {
                        eprintln!("error: {error}");
                        exit_code = 1;
                    }
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
                    WaitPoll::Failed(error) => {
                        eprintln!("error: {error}");
                        exit_code = 1;
                    }
                }
            }
            (ConsoleStep::Tui(Step::Quit), _) => break,
        }

        app.dirty = true;
        if draw(&mut app, &mut terminal, &theme).is_err() {
            exit_code = 1;
            break;
        }
        if let Err(error) = keep_frame(
            drive.frames_dir.as_deref(),
            terminal.backend(),
            &mut last_frame,
            &mut frame_index,
            pending_snap.take(),
        )
        .and_then(|()| keep_osc(drive.frames_dir.as_deref(), &mut app.osc_out))
        {
            eprintln!("error: {error}");
            exit_code = 1;
            break;
        }
    }

    outbound.shutdown();
    drop(events);
    join_with_deadline(threads);
    exit_code
}

fn configure(app: &mut App, options: &ConsoleOptions) {
    app.autostart = options.autostart;
    app.state.auto_side = options.auto_side;
    app.state.title_start = app.clock.now();
    app.state.title = options
        .title
        .as_ref()
        .and_then(|title| yi_tui::title::pick(&title.weights, title.seed));
    app.state.open_apps = options.open_apps.clone();
    app.state.sidebar = options.sidebar;
    app.state.chat_pace = options.pace;
}

/// What the console wrote outside the grid (clipboard, title), kept beside the frames; the
/// leading dot sorts it ahead of them, so the last file stays the last frame.
fn keep_osc(dir: Option<&std::path::Path>, osc: &mut Vec<String>) -> Result<(), String> {
    let Some(dir) = dir.filter(|_| !osc.is_empty()) else {
        return Ok(());
    };
    let text: String = osc.drain(..).map(|sequence| sequence + "\n").collect();
    let path = dir.join(".osc.log");
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, text.as_bytes()))
        .map_err(|error| format!("osc log {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Incident: a same-size resize now draws a clear, which takes the kitty image with it,
    /// while the cache still said placed, so the notebook image never came back.
    #[test]
    fn a_resize_places_the_notebook_image_again() -> Result<(), Box<dyn std::error::Error>> {
        let theme = Theme::new(ColorTier::TrueColor, true);
        let mut app = App::new("/repo".to_owned(), theme);
        let home = app.state.focused_pane_id().ok_or("a pane")?;
        if let Some(pane) = app.state.panes.get_mut(&home) {
            pane.content = crate::model::PaneContent::Notebook {
                session: None,
                cells: vec![crate::model::NbCell {
                    images: vec!["iVBORw0KGgo=".to_owned()],
                    ..crate::model::NbCell::default()
                }],
                full: false,
                drawn: None,
            };
        }
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 30))?;
        let placed = |app: &mut App| {
            let mut out = Vec::new();
            place_notebook_image(app, &mut out);
            String::from_utf8_lossy(&out).contains("a=p,")
        };
        draw(&mut app, &mut terminal, &theme)?;
        assert!(placed(&mut app), "placed once drawn");
        assert!(!placed(&mut app), "an unchanged frame writes nothing");
        let (events, _received) = client::events();
        let socket = std::env::temp_dir().join("yi-notebook-resize-no-daemon.sock");
        let (outbound, _threads) = client::spawn(socket, events);
        app.handle_event(&outbound, CtEvent::Resize(100, 30));
        outbound.shutdown();
        draw(&mut app, &mut terminal, &theme)?;
        assert!(placed(&mut app), "placed again after the clear");
        Ok(())
    }

    #[test]
    fn v_on_the_notebook_shows_the_code_rows_its_cap_cut() -> Result<(), Box<dyn std::error::Error>>
    {
        let theme = Theme::new(ColorTier::TrueColor, true);
        let mut app = App::new("/repo".to_owned(), theme);
        app.state.zone = crate::model::Zone::Panes;
        let home = app.state.focused_pane_id().ok_or("a pane")?;
        if let Some(pane) = app.state.panes.get_mut(&home) {
            pane.content = crate::model::PaneContent::Notebook {
                session: None,
                cells: vec![crate::model::NbCell {
                    number: 1,
                    code: (1..=30).map(|n| format!("x{n} = {n}\n")).collect(),
                    ..crate::model::NbCell::default()
                }],
                full: false,
                drawn: None,
            };
        }
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 40))?;
        let screen = |terminal: &Terminal<ratatui::backend::TestBackend>| -> String {
            let cells = terminal.backend().buffer().content();
            cells.iter().map(ratatui::buffer::Cell::symbol).collect()
        };
        draw(&mut app, &mut terminal, &theme)?;
        let before = screen(&terminal);
        assert!(before.contains("[…] code first 24 of 30 lines"), "{before}");
        assert!(!before.contains("x30 = 30"), "{before}");
        let (events, _received) = client::events();
        let socket = std::env::temp_dir().join("yi-notebook-full-no-daemon.sock");
        let (outbound, _threads) = client::spawn(socket, events);
        let v = ratatui::crossterm::event::KeyEvent::from(
            ratatui::crossterm::event::KeyCode::Char('v'),
        );
        app.handle_event(&outbound, CtEvent::Key(v));
        outbound.shutdown();
        draw(&mut app, &mut terminal, &theme)?;
        let after = screen(&terminal);
        assert!(after.contains("x30 = 30"), "{after}");
        assert!(!after.contains("[…]"), "{after}");
        Ok(())
    }

    /// Dies with an image written after the frame's update closed: a presented frame then
    /// showed it on its old rows over text that had moved, 19 times in one measured reply.
    #[test]
    fn a_frame_places_its_images_inside_its_own_update() -> Result<(), Box<dyn std::error::Error>> {
        let theme = Theme::new(ColorTier::TrueColor, true);
        let mut app = App::new("/repo".to_owned(), theme);
        app.kitty = true;
        let home = app.state.focused_pane_id().ok_or("a pane")?;
        if let Some(pane) = app.state.panes.get_mut(&home) {
            pane.content = crate::model::PaneContent::Notebook {
                session: None,
                cells: vec![crate::model::NbCell {
                    images: vec!["iVBORw0KGgo=".to_owned()],
                    ..crate::model::NbCell::default()
                }],
                full: false,
                drawn: None,
            };
        }
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 30))?;
        let mut out = Vec::new();
        present(
            &mut app,
            &mut terminal,
            &theme,
            &mut std::collections::HashMap::new(),
            &mut out,
        )?;
        let out = String::from_utf8_lossy(&out);
        let open = out.find("\u{1b}[?2026h").ok_or("no update opened")?;
        let place = out.find("a=p,").ok_or("the image was not placed")?;
        let close = out.rfind("\u{1b}[?2026l").ok_or("no update closed")?;
        assert!(open < place && place < close, "{out:?}");
        Ok(())
    }

    #[test]
    fn the_gear_is_placed_as_an_image_and_a_popup_hides_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let theme = Theme::new(ColorTier::TrueColor, true);
        let mut app = App::new("/repo".to_owned(), theme);
        app.kitty = true;
        app.state.link = crate::model::Link::Connected;
        app.state
            .split_focused(ratatui::layout::Direction::Horizontal);
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(100, 30))?;
        let mut out = Vec::new();
        present(
            &mut app,
            &mut terminal,
            &theme,
            &mut std::collections::HashMap::new(),
            &mut out,
        )?;
        let out = String::from_utf8_lossy(&out);
        assert_eq!(
            out.matches("c=3,r=1").count(),
            2,
            "a close image per pane: {out:?}"
        );
        let hints = app.hits.as_ref().map_or(0, |hits| hits.hint_buttons.len());
        assert!(hints >= 5, "the keys that fit are buttons: {hints}");
        let caps = 2 + 2 * hints;
        assert_eq!(
            out.matches("a=t,").count(),
            3 + caps,
            "gear, closers, dock and key caps"
        );
        assert!(out.contains("s=32,v=48,a=t,i=8000"), "{out:?}");
        assert!(out.contains("a=p,i=8000,p=1,q=2,C=1,c=4,r=3"), "{out:?}");
        let gear = app
            .hits
            .as_ref()
            .and_then(|hits| hits.gear)
            .ok_or("no gear")?;
        assert_eq!(gear.height, 3);
        if let Some(hits) = app.hits.as_mut() {
            hits.overlays = vec![gear];
        }
        let mut covered = Vec::new();
        place_avatars(&mut app, &mut covered);
        let covered = String::from_utf8_lossy(&covered);
        assert!(covered.contains("a=d,d=I,i=8000"), "{covered:?}");
        Ok(())
    }

    #[test]
    fn the_hovered_keycap_lightens_and_the_rest_keep_their_colour()
    -> Result<(), Box<dyn std::error::Error>> {
        let theme = Theme::new(ColorTier::TrueColor, true);
        let mut app = App::new("/repo".to_owned(), theme);
        app.state.link = crate::model::Link::Connected;
        let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 30))?;
        let key_bg = |terminal: &Terminal<ratatui::backend::TestBackend>,
                      at: ratatui::layout::Rect| {
            let cell = terminal
                .backend()
                .buffer()
                .cell((at.x.saturating_add(2), at.y));
            cell.map(|cell| cell.bg)
        };
        draw(&mut app, &mut terminal, &theme)?;
        let buttons = app
            .hits
            .as_ref()
            .map(|hits| hits.hint_buttons.clone())
            .unwrap_or_default();
        let (first, second) = (
            buttons.first().ok_or("no hint")?.0,
            buttons.get(1).ok_or("one hint")?.0,
        );
        let rest = (key_bg(&terminal, first), key_bg(&terminal, second));
        assert_ne!(rest.0, rest.1, "each keycap wears its own colour");
        let (events, _received) = client::events();
        let socket = std::env::temp_dir().join("yi-hover-no-daemon.sock");
        let (outbound, _threads) = client::spawn(socket, events);
        let moved = |column, row| {
            CtEvent::Mouse(ratatui::crossterm::event::MouseEvent {
                kind: ratatui::crossterm::event::MouseEventKind::Moved,
                column,
                row,
                modifiers: ratatui::crossterm::event::KeyModifiers::NONE,
            })
        };
        app.handle_event(&outbound, moved(second.x.saturating_add(4), second.y));
        outbound.shutdown();
        assert!(app.dirty, "entering a keycap redraws");
        draw(&mut app, &mut terminal, &theme)?;
        assert_eq!(
            key_bg(&terminal, first),
            rest.0,
            "a keycap the pointer is not on keeps its colour"
        );
        let lit = key_bg(&terminal, second).ok_or("no cell")?;
        assert_ne!(Some(lit), rest.1, "the hovered keycap lightens");
        Ok(())
    }

    /// Incident: the daemon names a session from its first prompt, so `BEL ESC]52;…` in it
    /// ended the title early and made the terminal write the clipboard.
    #[test]
    fn a_session_name_cannot_end_the_window_title() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = App::new("/repo".to_owned(), Theme::new(ColorTier::TrueColor, true));
        let id = SessionId("s-evil".to_owned());
        app.state.upsert_row(crate::model::SessionRow {
            id: id.clone(),
            root: "/repo".to_owned(),
            status: SessionStatus::Idle,
            attached: true,
            name: Some("Fix bug\u{7}\u{1b}]52;c;ZWNobyBwd25lZA==\u{7} now".to_owned()),
            created_ms: 0,
            last_ms: 0,
        });
        let home = app.state.focused_pane_id().ok_or("a pane")?;
        if let Some(pane) = app.state.panes.get_mut(&home) {
            pane.content = crate::model::PaneContent::Session {
                session: Some(id),
                chat: None,
            };
        }
        set_window_title(&mut app);
        let osc = app.osc_out.first().ok_or("no title written")?;
        let inner = osc
            .strip_prefix("\u{1b}]2;")
            .and_then(|rest| rest.strip_suffix('\u{7}'));
        assert_eq!(
            inner,
            Some("yi · Fix bug]52;c;ZWNobyBwd25lZA== now"),
            "{osc:?}"
        );
        Ok(())
    }
}
