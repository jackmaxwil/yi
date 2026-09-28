use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event as CtEvent, KeyCode, KeyEvent, KeyModifiers};
use yi_runtime::{AgentSession, SubagentHost};

use crate::app::{App, AskRequest, TuiOptions};
use crate::capture::{RecordingBackend, write_still};
use crate::colors::Theme;
use crate::keymap::{KeyCodeValue, SingleKey, default_keymap};

/// One step per line, `#` comments. `key <spec>` uses the keymap grammar, `type <text>` sends
/// characters, `wait <ms>` and `wait-idle <ms>` block (the latter times out red), `quit` exits.
#[derive(Debug, Clone)]
pub enum Step {
    Key(SingleKey),
    Type(String),
    Wait(u64),
    WaitIdle(u64),
    /// `wait-frame <ms> <text>` blocks until the frame shows `text`, or stops showing it when
    /// written `!text`. Timeout first so the text may hold spaces; the bool is the polarity.
    WaitFrame(u64, bool, String),
    /// `type-ms <n>` paces every later `type` step at one character per `n` ms. Zero, the
    /// default, keeps assertion scripts instant; a recording needs human typing speed.
    TypeMs(u64),
    Quit,
}

pub fn parse_script(source: &str) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    for (index, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let error = |message: String| format!("line {}: {message}", index + 1);
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let step = match word {
            "key" => Step::Key(SingleKey::parse(rest).map_err(error)?),
            "type" => Step::Type(rest.to_owned()),
            "wait" => Step::Wait(rest.parse().map_err(|e| error(format!("{e}")))?),
            "wait-idle" => Step::WaitIdle(rest.parse().map_err(|e| error(format!("{e}")))?),
            "wait-frame" => {
                let (ms, text) = rest
                    .split_once(' ')
                    .ok_or_else(|| error("wait-frame needs <ms> <text>".to_owned()))?;
                let (present, text) = match text.strip_prefix('!') {
                    Some(negated) => (false, negated),
                    None => (true, text),
                };
                Step::WaitFrame(
                    ms.parse().map_err(|e| error(format!("{e}")))?,
                    present,
                    text.to_owned(),
                )
            }
            "type-ms" => Step::TypeMs(rest.parse().map_err(|e| error(format!("{e}")))?),
            "quit" => Step::Quit,
            other => return Err(error(format!("unknown step: {other}"))),
        };
        steps.push(step);
    }
    Ok(steps)
}

pub fn key_event(key: &SingleKey) -> KeyEvent {
    let code = match key.code {
        KeyCodeValue::Char(c) => KeyCode::Char(c),
        KeyCodeValue::Enter => KeyCode::Enter,
        KeyCodeValue::Esc => KeyCode::Esc,
        KeyCodeValue::Tab => KeyCode::Tab,
        KeyCodeValue::Backspace => KeyCode::Backspace,
        KeyCodeValue::Delete => KeyCode::Delete,
        KeyCodeValue::Up => KeyCode::Up,
        KeyCodeValue::Down => KeyCode::Down,
        KeyCodeValue::Left => KeyCode::Left,
        KeyCodeValue::Right => KeyCode::Right,
        KeyCodeValue::Home => KeyCode::Home,
        KeyCodeValue::End => KeyCode::End,
        KeyCodeValue::PageUp => KeyCode::PageUp,
        KeyCodeValue::PageDown => KeyCode::PageDown,
        KeyCodeValue::Space => KeyCode::Char(' '),
        KeyCodeValue::F(n) => KeyCode::F(n),
    };
    let mut modifiers = KeyModifiers::empty();
    if key.ctrl {
        modifiers |= KeyModifiers::CONTROL;
    }
    if key.alt {
        modifiers |= KeyModifiers::ALT;
    }
    if key.shift {
        modifiers |= KeyModifiers::SHIFT;
    }
    KeyEvent::new(code, modifiers)
}

/// A day. `--deadline` is a convenience for paced proof runs, not a way to
/// park a headless process, and the seconds are attacker-free but unbounded.
const DEADLINE_CAP_SECS: u64 = 86_400;

pub fn typed_events(text: &str) -> Vec<CtEvent> {
    text.chars()
        .map(|ch| {
            CtEvent::Key(key_event(&SingleKey {
                code: KeyCodeValue::Char(ch),
                ctrl: false,
                alt: false,
                shift: false,
            }))
        })
        .collect()
}

pub enum WaitPoll {
    Done,
    Retry,
    TimedOut,
}

pub fn poll_condition(satisfied: bool, started: Instant, ms: u64, what: &str) -> WaitPoll {
    if satisfied {
        return WaitPoll::Done;
    }
    if started.elapsed() > Duration::from_millis(ms) {
        eprintln!("error: {what} timed out after {ms} ms");
        return WaitPoll::TimedOut;
    }
    std::thread::sleep(Duration::from_millis(2));
    WaitPoll::Retry
}

pub struct DriveOptions {
    pub script: Vec<Step>,
    pub frames_dir: Option<PathBuf>,
    /// asciicast v2 of the whole run, for `agg` to turn into a GIF.
    pub record: Option<PathBuf>,
    /// The last frame as a one-event cast, so `agg` renders the still with
    /// the same emulator as the recording.
    pub snap: Option<PathBuf>,
    /// Wall clock the whole script gets. A paced proof run legitimately
    /// outlives the default an assertion run needs.
    pub deadline_secs: u64,
    pub width: u16,
    pub height: u16,
}

/// The real loop — same App, reduce and draw path — over an in-memory screen,
/// with script steps for the keyboard. Draws dump to `frames_dir/NNNN.txt`.
pub fn run_headless(
    runtime: tokio::runtime::Runtime,
    session: Arc<AgentSession>,
    host: Arc<SubagentHost>,
    ask_rx: Receiver<AskRequest>,
    options: TuiOptions,
    drive: DriveOptions,
) -> i32 {
    let theme = Theme::new(crate::colors::ColorTier::Ansi16, true);
    let mut keymap = default_keymap();
    let overrides: Vec<(&str, &str)> = options
        .keys
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    if let Err(error) = keymap.apply_overrides(overrides) {
        eprintln!("error: keys config: {error}");
        return 2;
    }
    // One path for both sinks means the still truncates the recording that is
    // still open on it, and the run reports success having lost it.
    if let (Some(record), Some(snap)) = (&drive.record, &drive.snap)
        && std::path::absolute(record).ok() == std::path::absolute(snap).ok()
    {
        eprintln!("error: --record and --snap need different files");
        return 2;
    }
    let width = drive.width.max(20);
    let height = drive.height.max(8);
    let backend = match RecordingBackend::new(width, height, drive.record.as_deref()) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("error: recording: {error}");
            return 1;
        }
    };
    let mut terminal = match crate::terminal::Terminal::new(backend, 4.min(height - 1)) {
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

    let initial_prompt = options.initial_prompt.clone();
    let mut app = App::new(options, theme, keymap, usize::from(width));
    app.set_rows(usize::from(height));
    let (ui_rx, cmd_tx, runtime_thread) = crate::app::spawn_runtime_bridge(
        runtime,
        &session,
        &host,
        ask_rx,
        Duration::from_millis(16),
    );
    let mut port = Arc::clone(&session);
    app.load_history(&mut port);
    if let Some(prompt) = initial_prompt {
        app.note_submission();
        let _ = cmd_tx.send(crate::app::Command::Prompt(prompt));
    }

    let mut frame_index = 0_u32;
    let mut last_frame = String::new();
    let mut steps = drive.script.into_iter();
    let mut current: Option<(Step, Instant)> = None;
    let deadline_secs = drive.deadline_secs.clamp(1, DEADLINE_CAP_SECS);
    // `Instant + Duration` panics rather than saturating, and the seconds come
    // straight off the command line.
    let start = Instant::now();
    let mut type_ms = 0_u64;
    let mut exit_code = 0;

    while !app.is_quit() {
        if start.elapsed() > Duration::from_secs(deadline_secs) {
            eprintln!("error: drive script timed out ({deadline_secs} s wall)");
            exit_code = 1;
            break;
        }
        crate::port::tick(&mut app, &mut port, &ui_rx, &cmd_tx);
        crate::editor::process_pending_editor(&mut app, &mut terminal, false);

        let step = match current.take() {
            Some(pending) => pending,
            None => match steps.next() {
                Some(step) => (step, Instant::now()),
                None => break,
            },
        };
        match step {
            (Step::Key(key), _) => {
                app.handle_event(&cmd_tx, CtEvent::Key(key_event(&key)));
            }
            (Step::Type(text), _) => {
                for event in typed_events(&text) {
                    app.handle_event(&cmd_tx, event);
                    // Paced typing has to reach the screen character by character, or the
                    // recording still shows the whole line appearing at once.
                    if type_ms > 0 {
                        crate::render::draw(&mut app, &mut terminal, Some(&port));
                        std::thread::sleep(Duration::from_millis(type_ms));
                        // The step holds the outer loop, where the wall clock is read, so a
                        // long paced line would otherwise outrun the deadline unchecked.
                        if start.elapsed() > Duration::from_secs(deadline_secs) {
                            break;
                        }
                    }
                }
            }
            (Step::TypeMs(ms), _) => type_ms = ms,
            (Step::Wait(ms), started) => {
                if started.elapsed() < Duration::from_millis(ms) {
                    current = Some((Step::Wait(ms), started));
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            (Step::WaitIdle(ms), started) => {
                let idle = !(app.is_running() || app.awaiting_turn() || !app.has_run());
                match poll_condition(idle, started, ms, "wait-idle") {
                    WaitPoll::Done => {}
                    WaitPoll::Retry => current = Some((Step::WaitIdle(ms), started)),
                    WaitPoll::TimedOut => exit_code = 1,
                }
            }
            (Step::WaitFrame(ms, present, text), started) => {
                let shown = terminal.backend().screen().contains(&text) == present;
                match poll_condition(shown, started, ms, &format!("wait-frame {text:?}")) {
                    WaitPoll::Done => {}
                    WaitPoll::Retry => {
                        current = Some((Step::WaitFrame(ms, present, text), started))
                    }
                    WaitPoll::TimedOut => {
                        eprintln!("  waiting for {text:?}");
                        exit_code = 1;
                    }
                }
            }
            (Step::Quit, _) => break,
        }

        crate::render::draw(&mut app, &mut terminal, Some(&port));
        if let Some(dir) = &drive.frames_dir {
            let frame = terminal.backend().screen();
            if frame != last_frame {
                let path = dir.join(format!("{frame_index:04}.txt"));
                // A dump that silently failed to land reads downstream as a
                // frame that never differed.
                if let Err(error) = std::fs::write(&path, &frame) {
                    eprintln!("error: frame {}: {error}", path.display());
                    exit_code = 1;
                    break;
                }
                frame_index += 1;
                last_frame = frame;
            }
        }
        if let Some((x, y, symbol)) = terminal.backend().first_control_cell() {
            eprintln!("error: a control character reached a cell: {symbol:?} at ({x}, {y})");
            exit_code = 1;
            break;
        }
    }

    if let Some(path) = &drive.snap
        && let Err(error) = write_still(path, terminal.backend().buffer())
    {
        eprintln!("error: snap {}: {error}", path.display());
        exit_code = 1;
    }
    // The cast is buffered; an unflushed tail is a truncated recording.
    if let Err(error) = std::io::Write::flush(terminal.backend_mut()) {
        eprintln!("error: recording flush: {error}");
        exit_code = 1;
    }

    let _ = cmd_tx.send(crate::app::Command::Shutdown);
    let _ = runtime_thread.join();
    exit_code
}
