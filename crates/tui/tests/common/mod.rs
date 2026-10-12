use std::fmt;
use std::io::{self, Write};

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::prelude::CrosstermBackend;

/// Test backend (§17.3; ratatui 0.29 keeps
/// `CrosstermBackend::writer` private, so the parser sits behind a shared
/// handle): wraps a CrosstermBackend over a vt100::Parser to mock a real
/// terminal without ever touching stdout — size and cursor position come
/// from the parser.
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct SharedParser(
    Arc<Mutex<vt100::Parser>>,
    Arc<Mutex<Vec<u8>>>,
    Arc<Mutex<Scrolled>>,
);

/// Incident: vt100 drops rows a scroll region scrolls off; kitty and Ghostty keep them if the
/// region starts at row 0. Logs them as those do; rows moved by a bare linefeed are not seen.
#[derive(Default)]
struct Scrolled {
    scan: Scan,
    region: Option<(u16, u16)>,
    rows: Vec<String>,
}

/// Crossterm writes one escape in several pieces, so the scan outlives a single write.
#[derive(Default)]
enum Scan {
    #[default]
    Ground,
    Esc,
    Csi(Vec<u8>),
}

impl Scrolled {
    /// Feeds one byte; returns how many top rows a scroll-up is about to push off the screen.
    fn step(&mut self, byte: u8, screen_rows: u16) -> Option<usize> {
        let mut off = None;
        self.scan = match (std::mem::take(&mut self.scan), byte) {
            (_, 0x1b) => Scan::Esc,
            (Scan::Esc, b'[') => Scan::Csi(Vec::new()),
            (Scan::Csi(mut params), 0x20..=0x3f) if params.len() < 32 => {
                params.push(byte);
                Scan::Csi(params)
            }
            (Scan::Csi(params), b'r') => {
                self.set_region(&params, screen_rows);
                Scan::Ground
            }
            (Scan::Csi(params), b'S') => {
                off = self.scroll_off(&params, screen_rows);
                Scan::Ground
            }
            _ => Scan::Ground,
        };
        off
    }

    fn numbers(params: &[u8]) -> Option<Vec<u16>> {
        params
            .iter()
            .all(|b| b.is_ascii_digit() || *b == b';')
            .then(|| {
                params
                    .split(|b| *b == b';')
                    .map(|part| {
                        std::str::from_utf8(part)
                            .ok()
                            .and_then(|text| text.parse().ok())
                            .unwrap_or(0)
                    })
                    .collect()
            })
    }

    /// vt100's own rule: a region needs top < bottom, anything else is the full screen.
    fn set_region(&mut self, params: &[u8], screen_rows: u16) {
        let Some(numbers) = Self::numbers(params) else {
            return;
        };
        let at = |i: usize, default: u16| match numbers.get(i) {
            Some(0) | None => default,
            Some(n) => *n,
        };
        let top = at(0, 1).saturating_sub(1);
        let bottom = at(1, screen_rows)
            .saturating_sub(1)
            .min(screen_rows.saturating_sub(1));
        self.region = (top < bottom).then_some((top, bottom));
    }

    fn scroll_off(&self, params: &[u8], screen_rows: u16) -> Option<usize> {
        let count = match Self::numbers(params)?.first() {
            Some(0) | None => 1,
            Some(n) => usize::from(*n),
        };
        let (top, bottom) = self.region.unwrap_or((0, screen_rows.saturating_sub(1)));
        (top == 0).then(|| count.min(usize::from(bottom) + 1))
    }
}

impl Write for SharedParser {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let (Ok(mut parser), Ok(mut scrolled)) = (self.0.lock(), self.2.lock()) {
            let mut fed = 0;
            for (at, byte) in buf.iter().enumerate() {
                let (rows, width) = parser.screen().size();
                if let Some(count) = scrolled.step(*byte, rows) {
                    parser.process(buf.get(fed..at).unwrap_or_default());
                    fed = at;
                    scrolled
                        .rows
                        .extend(parser.screen().rows(0, width).take(count));
                }
            }
            parser.process(buf.get(fed..).unwrap_or_default());
        }
        if let Ok(mut log) = self.1.lock() {
            log.extend_from_slice(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct VT100Backend {
    parser: SharedParser,
    crossterm_backend: CrosstermBackend<SharedParser>,
}

impl VT100Backend {
    #[allow(dead_code)]
    pub fn new(width: u16, height: u16) -> Self {
        Self::with_scrollback(width, height, 0)
    }

    pub fn with_scrollback(width: u16, height: u16, scrollback_len: usize) -> Self {
        ratatui::crossterm::style::force_color_output(true);
        let parser = SharedParser(
            Arc::new(Mutex::new(vt100::Parser::new(
                height,
                width,
                scrollback_len,
            ))),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Scrolled::default())),
        );
        Self {
            parser: parser.clone(),
            crossterm_backend: CrosstermBackend::new(parser),
        }
    }

    pub fn contents(&self) -> String {
        self.parser
            .0
            .lock()
            .map(|parser| parser.screen().contents())
            .unwrap_or_default()
    }

    /// Simulate a terminal window resize: the emulator changes size under the
    /// running app, exactly as SIGWINCH does.
    #[allow(dead_code)]
    pub fn resize(&mut self, width: u16, height: u16) {
        if let Ok(mut parser) = self.parser.0.lock() {
            parser.set_size(height, width);
        }
        if let Ok(mut scrolled) = self.parser.2.lock() {
            scrolled.region = None;
        }
    }

    /// Every row that scrolled off the top, oldest first, then the visible screen.
    #[allow(dead_code)]
    pub fn transcript(&self) -> Vec<String> {
        let (Ok(parser), Ok(scrolled)) = (self.parser.0.lock(), self.parser.2.lock()) else {
            return Vec::new();
        };
        let width = parser.screen().size().1;
        let mut rows = scrolled.rows.clone();
        rows.extend(parser.screen().rows(0, width));
        rows
    }

    /// Drains the bytes written since the last call: what one frame actually
    /// sent the terminal, erases and all.
    #[allow(dead_code)]
    pub fn take_written(&mut self) -> String {
        let bytes = self
            .parser
            .1
            .lock()
            .map(|mut log| std::mem::take(&mut *log))
            .unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[allow(dead_code)]
    pub fn row_text(&self, row: u16) -> String {
        self.parser
            .0
            .lock()
            .map(|parser| {
                let screen = parser.screen();
                let width = screen.size().1;
                // A cell no frame wrote shows blank on a real screen, so it reads as a space.
                (0..width)
                    .filter_map(|col| screen.cell(row, col))
                    .filter(|cell| !cell.is_wide_continuation())
                    .map(|cell| match cell.contents() {
                        text if text.is_empty() => " ".to_owned(),
                        text => text,
                    })
                    .collect::<String>()
            })
            .unwrap_or_default()
    }

    fn cursor(&self) -> (u16, u16) {
        self.parser
            .0
            .lock()
            .map(|parser| parser.screen().cursor_position())
            .unwrap_or((0, 0))
    }

    fn screen_size(&self) -> (u16, u16) {
        self.parser
            .0
            .lock()
            .map(|parser| parser.screen().size())
            .unwrap_or((0, 0))
    }
}

impl Write for VT100Backend {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.parser.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.parser.flush()
    }
}

impl fmt::Display for VT100Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.contents())
    }
}

impl Backend for VT100Backend {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.crossterm_backend.draw(content)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.crossterm_backend.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.crossterm_backend.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        let (row, col) = self.cursor();
        Ok(Position::new(col, row))
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.crossterm_backend.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.crossterm_backend.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.crossterm_backend.clear_region(clear_type)
    }

    fn append_lines(&mut self, line_count: u16) -> io::Result<()> {
        self.crossterm_backend.append_lines(line_count)
    }

    fn size(&self) -> io::Result<Size> {
        let (rows, cols) = self.screen_size();
        Ok(Size::new(cols, rows))
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        let (rows, cols) = self.screen_size();
        Ok(WindowSize {
            columns_rows: Size::new(cols, rows),
            pixels: Size {
                width: 640,
                height: 480,
            },
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.parser.flush()
    }

    fn scroll_region_up(&mut self, region: std::ops::Range<u16>, scroll_by: u16) -> io::Result<()> {
        self.crossterm_backend.scroll_region_up(region, scroll_by)
    }

    fn scroll_region_down(
        &mut self,
        region: std::ops::Range<u16>,
        scroll_by: u16,
    ) -> io::Result<()> {
        self.crossterm_backend.scroll_region_down(region, scroll_by)
    }
}

/// A minimal catalog-free model for `TuiOptions`.
#[allow(dead_code, reason = "the shared harness serves several test binaries")]
pub fn test_model(id: &str) -> yi_types::model::Model {
    yi_types::model::Model {
        context_window: 128_000,
        max_tokens: 16_384,
        ..crate::fixture_model::model(id, "faux", "faux", "http://localhost:0", None)
    }
}

/// Replay a recorded session as the event stream the reducer sees live: each
/// assistant entry re-streamed through `stream_with_deltas`, each tool result
/// paired with the call it answers, and every turn a typed prompt opens is
/// closed. The §10 scrub masks the ids the branch walk chains through, so
/// entries are ordered by the file's own `seq` instead.
#[allow(dead_code, reason = "the shared harness serves several test binaries")]
pub fn replay_stream(
    app: &mut yi_tui::app::App,
    path: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use serde_json::Value;
    use std::collections::VecDeque;
    use yi_types::event::{AgentEvent, ToolResult};
    use yi_types::message::{AgentMessage, Attribution, Content};

    let text = std::fs::read_to_string(path)?;
    let mut entries: Vec<(u64, AgentMessage)> = Vec::new();
    for line in text.lines() {
        let row: Value = serde_json::from_str(line)?;
        let field = |key: &str| row.get(key).and_then(Value::as_str).map(str::to_owned);
        if (field("kind"), field("type"), field("lane"))
            != (
                Some("entry".to_owned()),
                Some("message".to_owned()),
                Some("main".to_owned()),
            )
        {
            continue;
        }
        let Some(message) = row.get("message") else {
            continue;
        };
        let seq = row.get("seq").and_then(Value::as_u64).unwrap_or(0);
        entries.push((seq, serde_json::from_value(message.clone())?));
    }
    entries.sort_by_key(|(seq, _)| *seq);

    let mut calls: VecDeque<(String, Value)> = VecDeque::new();
    let mut turn_open = false;
    for (_, message) in entries {
        match &message {
            AgentMessage::User { .. } => {
                if message.attribution() == Attribution::User {
                    if turn_open {
                        app.reduce_agent(AgentEvent::AgentEnd {
                            messages: Vec::new(),
                        });
                    }
                    app.reduce_agent(AgentEvent::AgentStart);
                    turn_open = true;
                }
                app.reduce_agent(AgentEvent::MessageStart {
                    message: message.clone(),
                });
            }
            AgentMessage::Assistant { content, .. } => {
                app.reduce_agent(AgentEvent::MessageStart {
                    message: message.clone(),
                });
                for event in yi_runtime::faux::stream_with_deltas(&message) {
                    app.reduce_agent(AgentEvent::MessageUpdate {
                        assistant_message_event: event,
                    });
                }
                app.reduce_agent(AgentEvent::MessageEnd {
                    message: message.clone(),
                });
                for block in content {
                    if let Content::ToolCall { id, arguments, .. } = block {
                        calls.push_back((id.clone(), Value::Object(arguments.clone())));
                    }
                }
            }
            AgentMessage::ToolResult {
                tool_call_id,
                tool_name,
                content,
                details,
                is_error,
                ..
            } => {
                let (id, args) = calls
                    .pop_front()
                    .unwrap_or_else(|| (tool_call_id.clone(), Value::Null));
                app.reduce_agent(AgentEvent::ToolExecutionStart {
                    tool_call_id: id.clone(),
                    tool_name: tool_name.clone(),
                    args,
                });
                app.reduce_agent(AgentEvent::ToolExecutionEnd {
                    tool_call_id: id,
                    tool_name: tool_name.clone(),
                    result: ToolResult {
                        content: content.clone(),
                        details: details.clone().unwrap_or(Value::Null),
                        usage: None,
                        added_tool_names: None,
                        terminate: None,
                    },
                    is_error: *is_error,
                });
            }
            AgentMessage::BashExecution { .. }
            | AgentMessage::Custom { .. }
            | AgentMessage::BranchSummary { .. }
            | AgentMessage::CompactionSummary { .. } => {}
        }
    }
    if turn_open {
        app.reduce_agent(AgentEvent::AgentEnd {
            messages: Vec::new(),
        });
    }
    Ok(())
}

/// The live region as rows of a real screen.
pub fn live_rows(app: &mut yi_tui::app::App) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut terminal = yi_tui::terminal::Terminal::new(VT100Backend::new(80, 24), 4)?;
    yi_tui::render::draw(app, &mut terminal, None);
    let backend = terminal.backend();
    Ok((0..24).map(|row| backend.row_text(row)).collect())
}
