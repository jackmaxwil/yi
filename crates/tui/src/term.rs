use std::borrow::Cow;
use std::fs::OpenOptions;
use std::io::{IsTerminal, Write};
use std::ops::Range;

use ratatui::backend::{ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    BeginSynchronizedUpdate, EndSynchronizedUpdate, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Position, Size};
use ratatui::prelude::CrosstermBackend;
use ratatui::text::Line;

use crate::terminal::Terminal;

/// `/dev/tty` when stdout is captured, so `$(yi …)` still gets a screen.
pub fn terminal_writer() -> std::io::Result<Box<dyn Write + Send>> {
    if std::io::stdout().is_terminal() {
        Ok(Box::new(std::io::stdout()))
    } else {
        let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        Ok(Box::new(tty))
    }
}

/// RAII terminal state. Every step `Drop` restores is applied here and restore failures
/// log rather than panic; the panic hook restores first so its message lands on a sane screen.
pub struct TerminalGuard;

impl TerminalGuard {
    pub fn new(writer: &mut impl Write) -> std::io::Result<Self> {
        enter_terminal(writer)?;
        Ok(Self)
    }
}

/// Raw mode plus the §17.3 flags without owning the restore: the external editor hands the
/// tty to a child and re-enters while the startup guard still owns restore-on-exit.
pub fn enter_terminal(writer: &mut impl Write) -> std::io::Result<()> {
    enable_raw_mode()?;
    execute!(writer, EnableBracketedPaste)?;
    // DISAMBIGUATE + REPORT_ALTERNATE_KEYS is the minimum for Shift+Enter
    // (design §17.3); REPORT_ALL_KEYS breaks paste on some terminals.
    let _ = execute!(
        writer,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        )
    );
    Ok(())
}

pub fn restore_terminal(writer: &mut impl Write) {
    if let Err(error) = execute!(writer, PopKeyboardEnhancementFlags) {
        eprintln!("yi: failed to pop keyboard flags: {error}");
    }
    if let Err(error) = execute!(writer, DisableBracketedPaste) {
        eprintln!("yi: failed to disable bracketed paste: {error}");
    }
    if let Err(error) = disable_raw_mode() {
        eprintln!("yi: failed to disable raw mode: {error}");
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        restore_terminal(&mut out);
    }
}

pub type Backend = ControlPictures<CrosstermBackend<Box<dyn Write + Send>>>;

/// OSC 2 with every control character dropped. Incident: a session named from its first prompt
/// held `BEL ESC]52;…`, which ended the title early and had the terminal write the clipboard.
pub fn window_title_osc(title: &str) -> String {
    let title: String = title.chars().filter(|c| !c.is_control()).collect();
    format!("\x1b]2;{title}\x07")
}

/// A control character as its Control Pictures glyph: U+2400 + c for C0, U+2421 for DEL,
/// and U+FFFD for a C1 control, which has no picture. Anything else is itself.
pub fn control_picture(c: char) -> char {
    match c {
        '\0'..='\u{1f}' => char::from_u32(0x2400 + u32::from(c)).unwrap_or('\u{fffd}'),
        '\u{7f}' => '\u{2421}',
        c if c.is_control() => '\u{fffd}',
        c => c,
    }
}

/// The real terminal's backend. Invariant: a control character in a cell reaches the terminal
/// as its glyph, one cell for one cell, so no body can move the cursor under ratatui's layout.
pub struct ControlPictures<B>(pub B);

impl<B: ratatui::backend::Backend> ratatui::backend::Backend for ControlPictures<B> {
    fn draw<'a, I>(&mut self, content: I) -> std::io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let cells: Vec<(u16, u16, Cow<'a, Cell>)> = content
            .map(|(x, y, cell)| {
                // One glyph for the cell: ratatui gives the CRLF grapheme one cell, not two.
                let Some(control) = cell.symbol().chars().find(|c| c.is_control()) else {
                    return (x, y, Cow::Borrowed(cell));
                };
                let mut shown = cell.clone();
                shown.set_symbol(control_picture(control).encode_utf8(&mut [0; 4]));
                (x, y, Cow::Owned(shown))
            })
            .collect();
        self.0
            .draw(cells.iter().map(|(x, y, cell)| (*x, *y, cell.as_ref())))
    }

    fn append_lines(&mut self, n: u16) -> std::io::Result<()> {
        self.0.append_lines(n)
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

    fn size(&self) -> std::io::Result<Size> {
        self.0.size()
    }

    fn window_size(&mut self) -> std::io::Result<WindowSize> {
        self.0.window_size()
    }

    fn flush(&mut self) -> std::io::Result<()> {
        ratatui::backend::Backend::flush(&mut self.0)
    }

    fn scroll_region_up(&mut self, region: Range<u16>, count: u16) -> std::io::Result<()> {
        self.0.scroll_region_up(region, count)
    }

    fn scroll_region_down(&mut self, region: Range<u16>, count: u16) -> std::io::Result<()> {
        self.0.scroll_region_down(region, count)
    }
}

impl<B: Write> Write for ControlPictures<B> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

/// Inline viewport anchored at the cursor; the height follows the live
/// region from here on ([`crate::terminal::Terminal::resize_viewport`]).
pub fn build_terminal(
    writer: Box<dyn Write + Send>,
    height: u16,
) -> std::io::Result<Terminal<Backend>> {
    Terminal::new(ControlPictures(CrosstermBackend::new(writer)), height)
}

/// The synchronized bracket lives on the whole frame ([`sync_frame`]), not here: bracketing
/// the commit alone presented a scrolled screen with the previous viewport under it.
pub fn commit_lines<B>(terminal: &mut Terminal<B>, lines: Vec<Line<'static>>) -> std::io::Result<()>
where
    B: ratatui::backend::Backend + Write,
{
    if lines.is_empty() {
        return Ok(());
    }
    let count = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    terminal.insert_before(count, |buf| {
        for (i, line) in lines.iter().enumerate() {
            let Ok(y) = u16::try_from(i) else { continue };
            buf.set_line(0, y, line, buf.area.width);
        }
    })?;
    Ok(())
}

/// One bracket around commit, viewport resize, reflow and draw, so the scroll and the new
/// viewport present together; a stale viewport under a scrolled screen flashes per paragraph.
pub fn sync_frame<B, R>(terminal: &mut Terminal<B>, frame: impl FnOnce(&mut Terminal<B>) -> R) -> R
where
    B: ratatui::backend::Backend + Write,
{
    let opened = execute!(terminal.backend_mut(), BeginSynchronizedUpdate).is_ok();
    let result = frame(terminal);
    if opened {
        let _ = execute!(terminal.backend_mut(), EndSynchronizedUpdate);
    }
    result
}
