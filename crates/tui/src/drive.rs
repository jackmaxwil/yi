use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::crossterm::event::{Event as CtEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Size};
use yi_runtime::{AgentSession, SubagentHost};

use crate::app::{App, AskRequest, TuiOptions, UiEvent};
use crate::colors::Theme;
use crate::keymap::{KeyCodeValue, SingleKey, default_keymap};

/// One step per line, `#` comments. `key <spec>` uses the keymap grammar,
/// `type <text>` sends characters, `wait <ms>`, `wait-idle <ms>` blocks until
/// the turn ends (times out red), `quit` exits.
#[derive(Debug, Clone)]
pub enum Step {
    Key(SingleKey),
    Type(String),
    Wait(u64),
    WaitIdle(u64),
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
            "quit" => Step::Quit,
            other => return Err(error(format!("unknown step: {other}"))),
        };
        steps.push(step);
    }
    Ok(steps)
}

fn key_event(key: &SingleKey) -> KeyEvent {
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

/// TestBackend with a no-op `Write`, so the shared draw/commit path (which
/// brackets real output in synchronized updates) accepts it unchanged.
pub struct HeadlessBackend(pub TestBackend);

impl Write for HeadlessBackend {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Backend for HeadlessBackend {
    fn draw<'a, I>(&mut self, content: I) -> std::io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.0.draw(content)
    }

    fn hide_cursor(&mut self) -> std::io::Result<()> {
        self.0.hide_cursor()
    }

    fn show_cursor(&mut self) -> std::io::Result<()> {
        self.0.show_cursor()
    }

    fn get_cursor_position(&mut self) -> std::io::Result<Position> {
        self.0.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> std::io::Result<()> {
        self.0.set_cursor_position(position)
    }

    fn clear(&mut self) -> std::io::Result<()> {
        self.0.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> std::io::Result<()> {
        self.0.clear_region(clear_type)
    }

    fn append_lines(&mut self, line_count: u16) -> std::io::Result<()> {
        self.0.append_lines(line_count)
    }

    fn size(&self) -> std::io::Result<Size> {
        self.0.size()
    }

    fn window_size(&mut self) -> std::io::Result<WindowSize> {
        self.0.window_size()
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Backend::flush(&mut self.0)
    }

    fn scroll_region_up(
        &mut self,
        region: std::ops::Range<u16>,
        scroll_by: u16,
    ) -> std::io::Result<()> {
        self.0.scroll_region_up(region, scroll_by)
    }

    fn scroll_region_down(
        &mut self,
        region: std::ops::Range<u16>,
        scroll_by: u16,
    ) -> std::io::Result<()> {
        self.0.scroll_region_down(region, scroll_by)
    }
}

pub struct DriveOptions {
    pub script: Vec<Step>,
    pub frames_dir: Option<PathBuf>,
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
    if keymap.apply_overrides(overrides).is_err() {
        return 2;
    }
    let width = drive.width.max(20);
    let height = drive.height.max(8);
    let mut terminal = match crate::terminal::Terminal::new(
        HeadlessBackend(TestBackend::new(width, height)),
        4.min(height - 1),
    ) {
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
    let (ui_tx, ui_rx, cmd_tx, handle, runtime_thread) =
        crate::app::spawn_runtime_bridge(runtime, &session);
    crate::app::replay_session(&mut app, &session);
    if let Some(prompt) = initial_prompt {
        app.note_submission();
        let _ = cmd_tx.send(crate::app::Command::Prompt(prompt));
    }

    let mut frame_index = 0_u32;
    let mut last_frame = String::new();
    let mut steps = drive.script.into_iter();
    let mut current: Option<(Step, Instant)> = None;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut exit_code = 0;

    while !app.is_quit() {
        if Instant::now() > deadline {
            eprintln!("error: drive script timed out (60 s wall)");
            exit_code = 1;
            break;
        }
        for ui_event in ui_rx.try_iter().collect::<Vec<_>>() {
            match ui_event {
                UiEvent::Agent(event) => app.reduce_agent(event),
                UiEvent::Child { child_id, event } => app.reduce_child(&child_id, event),
            }
        }
        for ask in ask_rx.try_iter().collect::<Vec<_>>() {
            app.open_approval(ask);
        }
        crate::app::sync_roster(&mut app, &host, &handle, &ui_tx);
        crate::app::process_pending_tree(&mut app, &session);
        crate::rewind::process_pending_rewind(&mut app, &mut terminal, &session);
        crate::rewind::process_pending_new(&mut app, &mut terminal, &session);
        crate::rewind::process_pending_undo(&mut app, &session);
        crate::commands::process_pending_selection(&mut app, &session);
        crate::commands::process_pending_command(&mut app, &session);
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
                for ch in text.chars() {
                    let key = SingleKey {
                        code: KeyCodeValue::Char(ch),
                        ctrl: false,
                        alt: false,
                        shift: false,
                    };
                    app.handle_event(&cmd_tx, CtEvent::Key(key_event(&key)));
                }
            }
            (Step::Wait(ms), started) => {
                if started.elapsed() < Duration::from_millis(ms) {
                    current = Some((Step::Wait(ms), started));
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            (Step::WaitIdle(ms), started) => {
                if app.is_running() || app.awaiting_turn() || !app.has_run() {
                    if started.elapsed() > Duration::from_millis(ms) {
                        eprintln!("error: wait-idle timed out after {ms} ms");
                        exit_code = 1;
                    } else {
                        current = Some((Step::WaitIdle(ms), started));
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
            }
            (Step::Quit, _) => break,
        }

        crate::render::draw(&mut app, &mut terminal, Some(&session));
        if let Some(dir) = &drive.frames_dir {
            let frame = terminal.backend().0.to_string();
            if frame != last_frame {
                let path = dir.join(format!("{frame_index:04}.txt"));
                if std::fs::write(&path, &frame).is_ok() {
                    frame_index += 1;
                    last_frame = frame;
                }
            }
        }
    }

    let _ = cmd_tx.send(crate::app::Command::Shutdown);
    let _ = runtime_thread.join();
    exit_code
}
