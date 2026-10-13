use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event as CtEvent, KeyCode, KeyEvent, KeyModifiers};
use yi_runtime::{AgentSession, SubagentHost};

use crate::app::{App, Command, TuiOptions, UiEvent};
use crate::approval::AskRequest;
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
    /// `snap <name>` keeps the frame drawn after this step as `snap-<name>.cast` and `.txt` in
    /// the frames dir: the moment a scene names, not wherever the script happened to end.
    Snap(String),
    /// `tick <ms>` steps the frozen clock a frame at a time, then draws once; `snap-strip <name>
    /// <n> <ms>` parses into `snap <name>-00`, `tick <ms>`, … `snap <name>-<n-1>`.
    Tick(u64),
    /// `hold <event type> [n]` stops the faux stream before the n-th event of that wire type
    /// (`text_delta`, `toolcall_start`, …) so a turn stays on screen mid-stream until `release`.
    Hold(String, u32),
    Release,
    Quit,
}

/// One frame of animation: a longer step would jump a reveal past what a person sees.
pub const TICK_STEP_MS: u64 = 16;
/// Ten minutes of scene time: a tick is stepped frame by frame, so a typo of a few zeros would
/// otherwise spin millions of frames.
const TICK_MAX_MS: u64 = 600_000;
const STRIP_MAX: u32 = 60;

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
            "snap" if is_snap_name(rest) => Step::Snap(rest.to_owned()),
            "snap" => return Err(error(format!("snap name {rest:?} is not [a-z0-9_-]+"))),
            "tick" => match rest.parse() {
                Ok(ms) if ms <= TICK_MAX_MS => Step::Tick(ms),
                Ok(ms) => return Err(error(format!("tick {ms} is past {TICK_MAX_MS} ms"))),
                Err(e) => return Err(error(format!("{e}"))),
            },
            "snap-strip" => {
                let mut parts = rest.split_whitespace();
                let (name, n, ms) = (parts.next(), parts.next(), parts.next());
                let (Some(name), Some(n), Some(ms), None) = (name, n, ms, parts.next()) else {
                    return Err(error("snap-strip needs <name> <n> <ms>".to_owned()));
                };
                let n: u32 = n.parse().map_err(|e| error(format!("{e}")))?;
                let ms: u64 = ms.parse().map_err(|e| error(format!("{e}")))?;
                if !is_snap_name(name) || !(1..=STRIP_MAX).contains(&n) || ms > TICK_MAX_MS {
                    return Err(error(format!(
                        "snap-strip wants a [a-z0-9_-]+ name and 1..={STRIP_MAX} frames"
                    )));
                }
                for frame in 0..n {
                    if frame > 0 {
                        steps.push(Step::Tick(ms));
                    }
                    steps.push(Step::Snap(format!("{name}-{frame:02}")));
                }
                continue;
            }
            "hold" => {
                let (kind, nth) = rest.split_once(' ').unwrap_or((rest, "1"));
                let nth: u32 = nth.trim().parse().map_err(|e| error(format!("{e}")))?;
                if nth == 0 {
                    return Err(error("hold counts events from 1".to_owned()));
                }
                if kind.is_empty() || !kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
                    return Err(error(format!("hold needs an event type, got {kind:?}")));
                }
                Step::Hold(kind.to_owned(), nth)
            }
            "release" => Step::Release,
            "quit" => Step::Quit,
            other => return Err(error(format!("unknown step: {other}"))),
        };
        steps.push(step);
    }
    refuse_repeated_snaps(steps.iter())?;
    Ok(steps)
}

/// A second `snap` of one name would overwrite the first frame without a word.
pub fn refuse_repeated_snaps<'a>(steps: impl Iterator<Item = &'a Step>) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for step in steps {
        if let Step::Snap(name) = step
            && !seen.insert(name)
        {
            return Err(format!(
                "snap {name} appears twice; each keyframe needs its own name"
            ));
        }
    }
    Ok(())
}

fn is_snap_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Dumps the frame drawn after a step when it changed, refuses one with a control character in
/// a cell, then writes the snap that step asked for.
pub fn keep_frame(
    dir: Option<&Path>,
    backend: &RecordingBackend,
    last: &mut String,
    index: &mut u32,
    snap: Option<String>,
) -> Result<(), String> {
    if let Some(dir) = dir {
        let frame = backend.screen();
        if frame != *last {
            let path = dir.join(format!("{index:04}.txt"));
            // A dump that silently failed to land reads downstream as a frame that never differed.
            std::fs::write(&path, &frame).map_err(|e| format!("frame {}: {e}", path.display()))?;
            *index = index.saturating_add(1);
            *last = frame;
        }
    }
    if let Some((x, y, symbol)) = backend.first_control_cell() {
        return Err(format!(
            "a control character reached a cell: {symbol:?} at ({x}, {y})"
        ));
    }
    snap.map_or(Ok(()), |name| write_snap(dir, &name, backend))
}

fn write_snap(dir: Option<&Path>, name: &str, backend: &RecordingBackend) -> Result<(), String> {
    let dir = dir.ok_or_else(|| format!("snap {name}: needs --frames"))?;
    let still = dir.join(format!("snap-{name}.cast"));
    write_still(&still, backend.buffer()).map_err(|e| format!("snap {}: {e}", still.display()))?;
    let text = dir.join(format!("snap-{name}.txt"));
    std::fs::write(&text, backend.screen()).map_err(|e| format!("snap {}: {e}", text.display()))
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
pub const DEADLINE_CAP_SECS: u64 = 86_400;

/// The wall clock a drive script gets: the seconds clamped to [1, DEADLINE_CAP_SECS]
/// against the moment the run began.
pub struct Deadline {
    secs: u64,
    start: Instant,
}

impl Deadline {
    pub fn new(requested_secs: u64) -> Self {
        // `Instant + Duration` panics rather than saturating, and the seconds
        // come straight off the command line.
        Self {
            secs: requested_secs.clamp(1, DEADLINE_CAP_SECS),
            start: Instant::now(),
        }
    }

    pub fn passed(&self) -> bool {
        self.start.elapsed() > Duration::from_secs(self.secs)
    }

    pub fn report(&self) {
        eprintln!("error: drive script timed out ({} s wall)", self.secs);
    }
}

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
    Failed(String),
}

pub fn poll_condition(satisfied: bool, started: Instant, ms: u64, what: &str) -> WaitPoll {
    if satisfied {
        return WaitPoll::Done;
    }
    if started.elapsed() > Duration::from_millis(ms) {
        return WaitPoll::Failed(format!("{what} timed out after {ms} ms"));
    }
    std::thread::sleep(Duration::from_millis(2));
    WaitPoll::Retry
}

const HOLD_WAIT_MS: u64 = 5_000;

pub fn timing_step(
    app: &mut App,
    port: &mut Arc<AgentSession>,
    ui_rx: &Receiver<UiEvent>,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    step: &Step,
    started: Instant,
) -> Option<WaitPoll> {
    let hold = Arc::clone(&port.provider().faux_hold);
    Some(match step {
        // Invariant: a frozen frame never depends on how much of a held stream arrived in real
        // time, so a tick waits for an armed hold to be reached first.
        Step::Tick(_) if hold.armed() && !hold.reached() => poll_condition(
            false,
            started,
            HOLD_WAIT_MS,
            "tick, the stream short of its hold,",
        ),
        Step::Tick(ms) => {
            if app.clock == crate::motion::Clock::Real {
                app.set_clock(crate::motion::Clock::Fixed(Instant::now()));
            }
            let mut left = *ms;
            while left > 0 {
                let frame = left.min(TICK_STEP_MS);
                // Incident: a step reduced in two halves revealed less than one reduced whole,
                // and a clock moved before the model request existed started its stream late.
                if let Some(error) = settle_faux(app, port, ui_rx, cmd_tx) {
                    return Some(WaitPoll::Failed(error));
                }
                app.advance_clock(Duration::from_millis(frame));
                port.provider()
                    .faux_time
                    .advance(Duration::from_millis(frame));
                if let Some(error) = settle_faux(app, port, ui_rx, cmd_tx) {
                    return Some(WaitPoll::Failed(error));
                }
                crate::port::tick(app, port, ui_rx, cmd_tx);
                left -= frame;
            }
            WaitPoll::Done
        }
        Step::Hold(kind, nth) => {
            hold.arm(kind, *nth);
            WaitPoll::Done
        }
        Step::Release if !hold.release() => {
            WaitPoll::Failed("release: the faux stream never reached its hold".to_owned())
        }
        Step::Release => {
            // The rest of the message lands before the next step, or a frozen reveal would
            // depend on how much arrived in real time.
            let settled = Instant::now();
            loop {
                crate::port::tick(app, port, ui_rx, cmd_tx);
                match poll_condition(app.stream_settled(), settled, HOLD_WAIT_MS, "release") {
                    WaitPoll::Retry => {}
                    done => break done,
                }
            }
        }
        Step::WaitIdle(ms) => match poll_condition(app.turn_idle(), started, *ms, "wait-idle") {
            WaitPoll::Failed(error) if app.clock != crate::motion::Clock::Real => WaitPoll::Failed(
                format!("{error}\n  the clock is frozen: `tick <ms>` lets a reveal finish"),
            ),
            poll => poll,
        },
        _ => return None,
    })
}

pub const CONTROL_POLL: Duration = Duration::from_millis(10);

/// `--control <socket>`: a harness typing keys into the real terminal sends the timing steps
/// here, one per line, each answered `ok` or `error: <why>` once done.
pub struct Control {
    stream: UnixStream,
    unread: Vec<u8>,
    step: Option<(Step, Instant)>,
}

impl Control {
    pub fn connect(path: &Path) -> std::io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            unread: Vec::new(),
            step: None,
        })
    }

    fn serve(
        &mut self,
        run: impl FnOnce(&Step, Instant) -> Option<WaitPoll>,
    ) -> std::io::Result<bool> {
        if self.step.is_none() {
            let mut chunk = [0_u8; 1024];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
                Ok(n) => self.unread.extend(chunk.iter().take(n)),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
            let Some(end) = self.unread.iter().position(|byte| *byte == b'\n') else {
                return Ok(false);
            };
            let line: Vec<u8> = self.unread.drain(..=end).collect();
            match parse_script(&String::from_utf8_lossy(&line)).map(Vec::into_iter) {
                Ok(mut steps) => match (steps.next(), steps.next()) {
                    (Some(step), None) => self.step = Some((step, Instant::now())),
                    _ => return self.answer(Err("one step per line".to_owned())),
                },
                Err(error) => return self.answer(Err(error)),
            }
        }
        let Some((step, started)) = self.step.take() else {
            return Ok(false);
        };
        match run(&step, started) {
            Some(WaitPoll::Retry) => {
                self.step = Some((step, started));
                Ok(false)
            }
            Some(WaitPoll::Done) => self.answer(Ok(())),
            Some(WaitPoll::Failed(error)) => self.answer(Err(error)),
            None => self.answer(Err(format!(
                "only the keyboard or the screen plays {step:?}"
            ))),
        }
    }

    fn answer(&mut self, result: Result<(), String>) -> std::io::Result<bool> {
        // Invariant: one line per answer, or an error's second line reads as the next answer.
        let line = result.map_or_else(
            |error| format!("error: {}\n", error.replace('\n', " ")),
            |()| "ok\n".to_owned(),
        );
        self.stream.write_all(line.as_bytes())?;
        Ok(true)
    }
}

pub fn serve(
    control: &mut Option<Control>,
    run: impl FnOnce(&Step, Instant) -> Option<WaitPoll>,
) -> bool {
    let Some(channel) = control.as_mut() else {
        return false;
    };
    channel.serve(run).unwrap_or_else(|_| {
        *control = None;
        false
    })
}

pub fn serve_solo(
    control: &mut Option<Control>,
    app: &mut App,
    port: &mut Arc<AgentSession>,
    ui_rx: &Receiver<UiEvent>,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
) {
    if serve(control, |step, at| {
        timing_step(app, port, ui_rx, cmd_tx, step, at)
    }) {
        app.scheduler.request();
    }
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

/// Waits until the screen has what the provider owes it; `None` once it does, or the reason it
/// gave up, after 5 s.
fn settle_faux(
    app: &mut App,
    port: &mut Arc<AgentSession>,
    ui_rx: &Receiver<UiEvent>,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
) -> Option<String> {
    let waiting = Instant::now();
    while app.awaiting_turn() {
        if waiting.elapsed() > Duration::from_secs(5) {
            return Some("tick: the turn did not start within 5 s".to_owned());
        }
        crate::port::tick(app, port, ui_rx, cmd_tx);
        std::thread::sleep(Duration::from_millis(1));
    }
    loop {
        app.pacing.receive(ui_rx);
        if !app.stream_owed(
            port.provider().faux_time.requests(),
            port.provider().faux_time.updates(),
        ) {
            return None;
        }
        if waiting.elapsed() > Duration::from_secs(5) {
            return Some("tick: the faux stream's updates did not arrive within 5 s".to_owned());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// The real loop — same App, reduce and draw path — over an in-memory screen,
/// with script steps for the keyboard. Draws dump to `frames_dir/NNNN.txt`.
pub fn run_headless(
    runtime: tokio::runtime::Runtime,
    session: Arc<AgentSession>,
    host: Arc<SubagentHost>,
    ask_rx: Receiver<AskRequest>,
    mut options: TuiOptions,
    drive: DriveOptions,
) -> i32 {
    let frozen = drive
        .script
        .iter()
        .any(|step| matches!(step, Step::Tick(_)));
    if frozen {
        // A frozen scene reveals text at the default pace, which only `tick` moves.
        options.pace = 100;
    }
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
    if frozen {
        app.set_clock(crate::motion::Clock::Fixed(Instant::now()));
        session.provider().faux_time.freeze();
    }
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
    let deadline = Deadline::new(drive.deadline_secs);
    let mut type_ms = 0_u64;
    let mut exit_code = 0;
    let mut pending_snap = None;

    while !app.is_quit() {
        if deadline.passed() {
            deadline.report();
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
                        if deadline.passed() {
                            break;
                        }
                    }
                }
            }
            (Step::TypeMs(ms), _) => type_ms = ms,
            (Step::Snap(name), _) => pending_snap = Some(name),
            (step @ (Step::Tick(_) | Step::Hold(..) | Step::Release | Step::WaitIdle(_)), at) => {
                match timing_step(&mut app, &mut port, &ui_rx, &cmd_tx, &step, at) {
                    Some(WaitPoll::Retry) => current = Some((step, at)),
                    Some(WaitPoll::Failed(error)) => {
                        eprintln!("error: {error}");
                        exit_code = 1;
                        break;
                    }
                    Some(WaitPoll::Done) | None => {}
                }
            }
            (Step::Wait(ms), started) => {
                if started.elapsed() < Duration::from_millis(ms) {
                    current = Some((Step::Wait(ms), started));
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            (Step::WaitFrame(ms, present, text), started) => {
                let shown = terminal.backend().screen().contains(&text) == present;
                match poll_condition(shown, started, ms, &format!("wait-frame {text:?}")) {
                    WaitPoll::Done => {}
                    WaitPoll::Retry => {
                        current = Some((Step::WaitFrame(ms, present, text), started))
                    }
                    WaitPoll::Failed(error) => {
                        eprintln!("error: {error}\n  waiting for {text:?}");
                        exit_code = 1;
                    }
                }
            }
            (Step::Quit, _) => break,
        }

        crate::render::draw(&mut app, &mut terminal, Some(&port));
        if let Err(error) = keep_frame(
            drive.frames_dir.as_deref(),
            terminal.backend(),
            &mut last_frame,
            &mut frame_index,
            pending_snap.take(),
        ) {
            eprintln!("error: {error}");
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
