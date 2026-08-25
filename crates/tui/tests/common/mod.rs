use std::fmt;
use std::io::{self, Write};

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::prelude::CrosstermBackend;

/// codex `test_backend.rs:1-135`, port adapted (U19; ratatui 0.29 keeps
/// `CrosstermBackend::writer` private, so the parser sits behind a shared
/// handle): wraps a CrosstermBackend over a vt100::Parser to mock a real
/// terminal without ever touching stdout — size and cursor position come
/// from the parser.
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct SharedParser(Arc<Mutex<vt100::Parser>>);

impl Write for SharedParser {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Ok(mut parser) = self.0.lock() {
            parser.process(buf);
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
        let parser = SharedParser(Arc::new(Mutex::new(vt100::Parser::new(
            height,
            width,
            scrollback_len,
        ))));
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
