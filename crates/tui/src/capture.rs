//! Recording sinks for a headless drive run: the draws the script asserts on,
//! mirrored through a `CrosstermBackend` into an asciicast v2 file `agg` turns
//! into a GIF, plus `buffer_to_ansi` for a still.

// Crossterm answers `size`, `window_size` and cursor position from the real
// tty, which headless has none of, so reads never reach it. And the loop
// redraws every few milliseconds: a call carrying no change is not recorded.

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use ratatui::backend::{Backend, ClearType, CrosstermBackend, TestBackend, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Size};

/// Crossterm strips colour when its writer is not a terminal, which would
/// leave every recording monochrome.
fn force_color() {
    ratatui::crossterm::style::force_color_output(true);
}

/// A `Write` whose bytes are readable after the backend owning it is done:
/// ratatui 0.29 gates `CrosstermBackend::writer` behind an unstable feature.
#[derive(Clone, Default)]
struct SharedBuf(Rc<RefCell<Vec<u8>>>);

impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .try_borrow_mut()
            .map_err(io::Error::other)?
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// An asciicast v2 file (docs.asciinema.org/manual/asciicast/v2): a JSON
/// header line, then `[time, "o", data]` per frame. Newline-delimited, so a
/// killed run still leaves a playable prefix.
pub struct CastWriter {
    out: BufWriter<File>,
    start: Instant,
    /// Bytes since the last flush. A character split across a write or a
    /// flush waits here: asciicast payloads are JSON strings.
    buf: Vec<u8>,
}

impl CastWriter {
    pub fn create(path: &Path, width: u16, height: u16) -> io::Result<Self> {
        let mut out = BufWriter::new(File::create(path)?);
        let header = serde_json::json!({
            "version": 2,
            "width": width,
            "height": height,
            "env": { "TERM": "xterm-256color" },
        });
        writeln!(out, "{header}")?;
        Ok(Self {
            out,
            start: Instant::now(),
            buf: Vec::new(),
        })
    }

    fn event(&mut self, payload: &str) -> io::Result<()> {
        // The synchronized-update bracket asks a live terminal to present a
        // frame atomically: meaningless here, an event on every idle tick,
        // and a player left mid-update when its closing half is dropped.
        let payload = payload
            .replace("\x1b[?2026h", "")
            .replace("\x1b[?2026l", "");
        if payload.is_empty() {
            return Ok(());
        }
        let line = serde_json::json!([self.start.elapsed().as_secs_f64(), "o", payload]);
        writeln!(self.out, "{line}")
    }

    /// The valid prefix of `buf`, as an owned string, with those bytes plus
    /// `skip` more removed. Owned because emitting it needs `&mut self`.
    fn take_prefix(&mut self, valid: usize, skip: usize) -> String {
        let text = self
            .buf
            .get(..valid)
            .and_then(|head| std::str::from_utf8(head).ok())
            .unwrap_or_default()
            .to_owned();
        let drop_to = valid.saturating_add(skip).min(self.buf.len());
        self.buf.drain(..drop_to);
        text
    }
}

impl Write for CastWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    // The event boundary: crossterm formats commands piecewise, so one event
    // per write turned a three-frame run into 431 single-character events.
    fn flush(&mut self) -> io::Result<()> {
        loop {
            // No `error_len` means the frame ends mid-character, so the tail
            // waits; `Some(n)` is invalid and is dropped rather than stalling.
            let (valid, skip, done) = match std::str::from_utf8(&self.buf) {
                Ok(text) => (text.len(), 0, true),
                Err(error) => match error.error_len() {
                    None => (error.valid_up_to(), 0, true),
                    Some(bad) => (error.valid_up_to(), bad, false),
                },
            };
            let text = self.take_prefix(valid, skip);
            self.event(&text)?;
            if done {
                break;
            }
        }
        self.out.flush()
    }
}

/// The drive loop's screen: a `TestBackend` for assertions, optionally
/// mirrored into a cast file.
pub struct RecordingBackend {
    inner: TestBackend,
    cast: Option<CrosstermBackend<CastWriter>>,
    cursor_hidden: bool,
}

impl RecordingBackend {
    pub fn new(width: u16, height: u16, record: Option<&Path>) -> io::Result<Self> {
        force_color();
        let cast = match record {
            Some(path) => Some(CrosstermBackend::new(CastWriter::create(
                path, width, height,
            )?)),
            None => None,
        };
        Ok(Self {
            inner: TestBackend::new(width, height),
            cast,
            cursor_hidden: false,
        })
    }

    /// The rendered screen as text — what `wait-frame` matches and what the
    /// frame dumps hold.
    pub fn screen(&self) -> String {
        self.inner.to_string()
    }

    pub const fn buffer(&self) -> &Buffer {
        self.inner.buffer()
    }
}

impl Write for RecordingBackend {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // OSC titles, prompt zones and the sync bracket bypass the buffer
        // diff: no use to the assertion backend, wanted in a recording.
        if let Some(cast) = &mut self.cast {
            cast.write_all(buf)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.cast {
            Some(cast) => Write::flush(cast),
            None => Ok(()),
        }
    }
}

impl Backend for RecordingBackend {
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let updates: Vec<(u16, u16, &Cell)> = content.collect();
        self.inner.draw(updates.iter().copied())?;
        // Crossterm ends even an empty `draw` with a reset triple, and the
        // loop draws on every tick.
        if updates.is_empty() {
            return Ok(());
        }
        if let Some(cast) = &mut self.cast {
            cast.draw(updates.into_iter())?;
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()?;
        // terminal.rs hides the cursor after every draw, changed frame or not.
        if std::mem::replace(&mut self.cursor_hidden, true) {
            return Ok(());
        }
        if let Some(cast) = &mut self.cast {
            cast.hide_cursor()?;
        }
        Ok(())
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()?;
        if !std::mem::replace(&mut self.cursor_hidden, false) {
            return Ok(());
        }
        if let Some(cast) = &mut self.cast {
            cast.show_cursor()?;
        }
        Ok(())
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let position = position.into();
        self.inner.set_cursor_position(position)?;
        if let Some(cast) = &mut self.cast {
            cast.set_cursor_position(position)?;
        }
        Ok(())
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()?;
        if let Some(cast) = &mut self.cast {
            cast.clear()?;
        }
        Ok(())
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)?;
        if let Some(cast) = &mut self.cast {
            cast.clear_region(clear_type)?;
        }
        Ok(())
    }

    fn append_lines(&mut self, line_count: u16) -> io::Result<()> {
        self.inner.append_lines(line_count)?;
        if let Some(cast) = &mut self.cast {
            cast.append_lines(line_count)?;
        }
        Ok(())
    }

    fn size(&self) -> io::Result<Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)?;
        match &mut self.cast {
            Some(cast) => Backend::flush(cast),
            None => Ok(()),
        }
    }

    fn scroll_region_up(&mut self, region: std::ops::Range<u16>, scroll_by: u16) -> io::Result<()> {
        self.inner.scroll_region_up(region.clone(), scroll_by)?;
        if let Some(cast) = &mut self.cast {
            cast.scroll_region_up(region, scroll_by)?;
        }
        Ok(())
    }

    fn scroll_region_down(
        &mut self,
        region: std::ops::Range<u16>,
        scroll_by: u16,
    ) -> io::Result<()> {
        self.inner.scroll_region_down(region.clone(), scroll_by)?;
        if let Some(cast) = &mut self.cast {
            cast.scroll_region_down(region, scroll_by)?;
        }
        Ok(())
    }
}

// Rows are placed, not newline-terminated, because the reader is a terminal:
// ending 24 rows with a newline on a 24-row screen scrolls the frame off
// itself, measured, and the still came back blank.

/// A whole frame as the escape stream a terminal would receive to paint it:
/// every cell placed by absolute cursor move, no newlines.
pub fn buffer_to_ansi(buffer: &Buffer) -> io::Result<Vec<u8>> {
    force_color();
    let area = buffer.area();
    let mut cells = Vec::new();
    for y in 0..area.height {
        for x in 0..area.width {
            let source = Position::new(area.x.saturating_add(x), area.y.saturating_add(y));
            if let Some(cell) = buffer.cell(source) {
                cells.push((x, y, cell));
            }
        }
    }
    let sink = SharedBuf::default();
    let mut backend = CrosstermBackend::new(sink.clone());
    backend.write_all(b"\x1b[0m\x1b[H")?;
    backend.draw(cells.into_iter())?;
    Write::flush(&mut backend)?;
    let bytes = sink.0.try_borrow().map_err(io::Error::other)?.clone();
    Ok(bytes)
}

/// The final frame as a cast of its own, so the still and the recording are
/// rendered by the same terminal emulator and agree pixel for pixel.
pub fn write_still(path: &Path, buffer: &Buffer) -> io::Result<()> {
    let area = buffer.area();
    let mut cast = CastWriter::create(path, area.width, area.height)?;
    cast.write_all(&buffer_to_ansi(buffer)?)?;
    Write::flush(&mut cast)
}
