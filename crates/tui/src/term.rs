use std::fs::OpenOptions;
use std::io::{IsTerminal, Write};

use ratatui::crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    BeginSynchronizedUpdate, EndSynchronizedUpdate, disable_raw_mode, enable_raw_mode,
};
use ratatui::prelude::CrosstermBackend;
use ratatui::text::Line;

use crate::terminal::Terminal;

/// `/dev/tty` when stdout is captured, so `$(yi …)` still gets a screen
/// (atuin `TerminalWriter`, adapted).
pub fn terminal_writer() -> std::io::Result<Box<dyn Write + Send>> {
    if std::io::stdout().is_terminal() {
        Ok(Box::new(std::io::stdout()))
    } else {
        let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        Ok(Box::new(tty))
    }
}

/// U1: RAII terminal state. Every step `Drop` restores is applied here;
/// failures on restore are logged, never panicked — the panic hook calls
/// `restore_terminal` first so a panic message lands on a sane screen.
pub struct TerminalGuard;

impl TerminalGuard {
    pub fn new(writer: &mut impl Write) -> std::io::Result<Self> {
        enter_terminal(writer)?;
        Ok(Self)
    }
}

/// Raw mode plus the U1 flags, without taking ownership of the restore: the
/// external editor (U18) hands the tty to a child and re-enters afterwards
/// while the startup guard still owns restore-on-exit.
pub fn enter_terminal(writer: &mut impl Write) -> std::io::Result<()> {
    enable_raw_mode()?;
    execute!(writer, EnableBracketedPaste)?;
    // DISAMBIGUATE + REPORT_ALTERNATE_KEYS is the minimum for Shift+Enter
    // (design U1); REPORT_ALL_KEYS breaks paste on some terminals.
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

pub type Backend = CrosstermBackend<Box<dyn Write + Send>>;

/// U2: inline viewport anchored at the cursor; the height follows the live
/// region from here on (`Terminal::set_viewport_height`).
pub fn build_terminal(
    writer: Box<dyn Write + Send>,
    height: u16,
) -> std::io::Result<Terminal<Backend>> {
    Terminal::new(CrosstermBackend::new(writer), height)
}

/// U3: commit finished cells above the viewport, batched inside a
/// synchronized-output bracket (codex `tui.rs:929-971`).
pub fn commit_lines<B>(terminal: &mut Terminal<B>, lines: Vec<Line<'static>>) -> std::io::Result<()>
where
    B: ratatui::backend::Backend + Write,
{
    if lines.is_empty() {
        return Ok(());
    }
    execute!(terminal.backend_mut(), BeginSynchronizedUpdate)?;
    let count = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    terminal.insert_before(count, |buf| {
        for (i, line) in lines.iter().enumerate() {
            let Ok(y) = u16::try_from(i) else { continue };
            buf.set_line(0, y, line, buf.area.width);
        }
    })?;
    execute!(terminal.backend_mut(), EndSynchronizedUpdate)?;
    Ok(())
}
