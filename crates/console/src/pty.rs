//! A shell in a pane: the console holds the PTY master and parses it with `vt100`; the child is
//! `yi __pty <shell>`, which takes the slave as its controlling terminal and execs the shell.

use std::ffi::OsString;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::client::ClientEvent;
use crate::render::short_path;

const SCROLLBACK_ROWS: usize = 5_000;
const WRITE_QUEUE: usize = 64;

pub struct Pty {
    master: File,
    writes: mpsc::SyncSender<Vec<u8>>,
    faults: Arc<Mutex<Option<std::io::Error>>>,
    parser: Arc<Mutex<vt100::Parser>>,
    fresh: Arc<AtomicBool>,
    child: Option<Child>,
    exited: Option<String>,
    size: (u16, u16),
    title: String,
}

fn winsize(rows: u16, cols: u16) -> rustix::termios::Winsize {
    rustix::termios::Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

impl Pty {
    #[expect(
        clippy::disallowed_methods,
        reason = "the terminal pane runs the user's own shell, through this binary's PTY shim"
    )]
    pub fn spawn(
        exe: &Path,
        shell: &str,
        cwd: &str,
        wake: Option<SyncSender<ClientEvent>>,
    ) -> std::io::Result<Self> {
        use std::os::unix::ffi::OsStringExt;
        let master = rustix::pty::openpt(rustix::pty::OpenptFlags::RDWR)?;
        rustix::io::fcntl_setfd(&master, rustix::io::FdFlags::CLOEXEC)?;
        rustix::pty::grantpt(&master)?;
        rustix::pty::unlockpt(&master)?;
        let name = OsString::from_vec(rustix::pty::ptsname(&master, Vec::new())?.into_bytes());
        let slave = File::options().read(true).write(true).open(name)?;
        let (rows, cols) = (24, 80);
        rustix::termios::tcsetwinsize(&master, winsize(rows, cols))?;
        let child = Command::new(exe)
            .arg("__pty")
            .arg(shell)
            .current_dir(cwd)
            .env("TERM", "xterm-256color")
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave))
            .spawn()?;
        let master = File::from(master);
        let mut reader = master.try_clone()?;
        let mut writer = master.try_clone()?;
        let (writes, pending) = mpsc::sync_channel::<Vec<u8>>(WRITE_QUEUE);
        let faults = Arc::new(Mutex::new(None));
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK_ROWS)));
        let fresh = Arc::new(AtomicBool::new(true));
        let (feed, flag) = (Arc::clone(&parser), Arc::clone(&fresh));
        std::thread::spawn(move || {
            let mut buf = [0_u8; 8192];
            loop {
                let read = match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(read) => read,
                    Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => break,
                };
                lock(&feed).process(buf.get(..read).unwrap_or_default());
                // Invariant: a full queue already wakes the loop, and its tick reads `fresh`.
                if !flag.swap(true, Ordering::AcqRel)
                    && let Some(wake) = &wake
                {
                    let _ = wake.try_send(ClientEvent::Wake);
                }
            }
        });
        let slot = Arc::clone(&faults);
        std::thread::spawn(move || {
            for bytes in pending {
                if let Err(error) = writer.write_all(&bytes) {
                    *lock(&slot) = Some(error);
                    break;
                }
            }
        });
        Ok(Self {
            master,
            writes,
            faults,
            parser,
            fresh,
            child: Some(child),
            exited: None,
            size: (rows, cols),
            title: format!(">_ {} · {}", short_path(shell), short_path(cwd)),
        })
    }

    pub fn exited(&self) -> bool {
        self.exited.is_some()
    }

    /// Invariant: the console loop never writes the master, and the write queue behind it is
    /// capped; a child that stops reading fails the next send visibly instead of growing memory.
    pub fn send(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if let Some(error) = lock(&self.faults).take() {
            return Err(error);
        }
        match self.writes.try_send(bytes.to_vec()) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(_)) => Err(std::io::Error::new(
                ErrorKind::WouldBlock,
                "input queue full; the child is not reading",
            )),
            Err(mpsc::TrySendError::Disconnected(_)) => Err(std::io::Error::new(
                ErrorKind::BrokenPipe,
                "terminal writer stopped",
            )),
        }
    }

    pub fn paste(&mut self, text: &str) -> std::io::Result<()> {
        let text = paste_text(text);
        if lock(&self.parser).screen().bracketed_paste() {
            self.send(format!("\x1b[200~{text}\x1b[201~").as_bytes())
        } else {
            self.send(text.as_bytes())
        }
    }

    pub fn application_cursor(&self) -> bool {
        lock(&self.parser).screen().application_cursor()
    }

    pub fn poll(&mut self) -> bool {
        use std::os::unix::process::ExitStatusExt;
        let fresh = self.fresh.swap(false, Ordering::AcqRel);
        let status = self.child.as_mut().and_then(|child| child.try_wait().ok()?);
        match status {
            Some(status) if self.exited.is_none() => {
                self.exited = Some(status.code().map_or_else(
                    || format!("signal {}", status.signal().unwrap_or_default()),
                    |code| code.to_string(),
                ));
                self.child = None;
                true
            }
            _ => fresh,
        }
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        if rows == 0 || cols == 0 || self.size == (rows, cols) {
            return;
        }
        self.size = (rows, cols);
        let _ = rustix::termios::tcsetwinsize(&self.master, winsize(rows, cols));
        lock(&self.parser).set_size(rows, cols);
    }

    pub fn view(&mut self, inner: Rect, scroll: &mut usize) -> (String, Vec<Line<'static>>) {
        self.resize(inner.height, inner.width);
        let mut parser = lock(&self.parser);
        parser.set_scrollback(*scroll);
        let screen = parser.screen();
        *scroll = screen.scrollback();
        let (rows, cols) = screen.size();
        let lines = (0..rows).map(|row| row_line(screen, row, cols)).collect();
        let mut title = self.title.clone();
        if *scroll > 0 {
            title.push_str(&format!(" · ↑{scroll}"));
        }
        if let Some(code) = &self.exited {
            title.push_str(&format!(" [exited {code}]"));
        }
        (title, lines)
    }

    pub fn cursor(&self, inner: Rect) -> Option<(u16, u16)> {
        let parser = lock(&self.parser);
        let screen = parser.screen();
        if self.exited() || screen.hide_cursor() || screen.scrollback() > 0 {
            return None;
        }
        let (row, col) = screen.cursor_position();
        let (x, y) = (inner.x.checked_add(col)?, inner.y.checked_add(row)?);
        (x < inner.right() && y < inner.bottom()).then_some((x, y))
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let group = rustix::process::Pid::from_child(&child);
        if rustix::process::kill_process_group(group, rustix::process::Signal::Hup).is_err() {
            let _ = child.kill();
        }
        // Invariant: a dropped Child is never reaped; a thread waits the shell out.
        std::thread::spawn(move || child.wait());
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn row_line(screen: &vt100::Screen, row: u16, cols: u16) -> Line<'static> {
    let mut spans = Vec::new();
    let (mut text, mut style) = (String::new(), Style::default());
    for col in 0..cols {
        let Some(cell) = screen.cell(row, col) else {
            break;
        };
        if cell.is_wide_continuation() {
            continue;
        }
        let next = style_of(cell);
        if next != style && !text.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut text), style));
        }
        style = next;
        match cell.contents() {
            contents if contents.is_empty() => text.push(' '),
            contents => text.push_str(&contents),
        }
    }
    spans.push(Span::styled(text, style));
    Line::from(spans)
}

fn color(color: vt100::Color) -> Option<Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(index) => Some(Color::Indexed(index)),
        vt100::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}

fn style_of(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();
    if let Some(fg) = color(cell.fgcolor()) {
        style = style.fg(fg);
    }
    if let Some(bg) = color(cell.bgcolor()) {
        style = style.bg(bg);
    }
    for (on, modifier) in [
        (cell.bold(), Modifier::BOLD),
        (cell.italic(), Modifier::ITALIC),
        (cell.underline(), Modifier::UNDERLINED),
        (cell.inverse(), Modifier::REVERSED),
    ] {
        if on {
            style = style.add_modifier(modifier);
        }
    }
    style
}

fn control(c: char) -> Option<u8> {
    match c.to_ascii_lowercase() {
        letter @ 'a'..='z' => u8::try_from(letter).ok().map(|byte| byte & 0x1f),
        '@' | ' ' => Some(0),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

/// The bytes an xterm sends for `key`: modified cursor and editing keys as `CSI 1;m`, alt as an
/// ESC prefix; `app_cursor` is the child's DECCKM, which moves plain arrows to `SS3`.
pub fn encode(key: &KeyEvent, app_cursor: bool) -> Vec<u8> {
    if key.kind == KeyEventKind::Release {
        return Vec::new();
    }
    let has = |modifier| key.modifiers.contains(modifier);
    let (alt, ctrl) = (has(KeyModifiers::ALT), has(KeyModifiers::CONTROL));
    let modifier = 1 + u8::from(has(KeyModifiers::SHIFT)) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let cursor = |end: char| match (modifier, app_cursor) {
        (1, true) => format!("\x1bO{end}"),
        (1, false) => format!("\x1b[{end}"),
        _ => format!("\x1b[1;{modifier}{end}"),
    };
    let tilde = |code: u8| match modifier {
        1 => format!("\x1b[{code}~"),
        _ => format!("\x1b[{code};{modifier}~"),
    };
    let modified = match key.code {
        KeyCode::Up => Some(cursor('A')),
        KeyCode::Down => Some(cursor('B')),
        KeyCode::Right => Some(cursor('C')),
        KeyCode::Left => Some(cursor('D')),
        KeyCode::Home => Some(cursor('H')),
        KeyCode::End => Some(cursor('F')),
        KeyCode::Insert => Some(tilde(2)),
        KeyCode::Delete => Some(tilde(3)),
        KeyCode::PageUp => Some(tilde(5)),
        KeyCode::PageDown => Some(tilde(6)),
        KeyCode::F(n @ 1..=4) => Some(format!("\x1bO{}", char::from(b'O'.saturating_add(n)))),
        KeyCode::F(n) => [15, 17, 18, 19, 20, 21, 23, 24]
            .get(usize::from(n).saturating_sub(5))
            .map(|code| tilde(*code)),
        _ => None,
    };
    if let Some(sequence) = modified {
        return sequence.into_bytes();
    }
    let body = match key.code {
        KeyCode::Char(c) if ctrl => {
            control(c).map_or_else(|| c.to_string().into_bytes(), |b| vec![b])
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace if ctrl => vec![0x08],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => vec![0x1b],
        _ => return Vec::new(),
    };
    if alt {
        [&[0x1b][..], &body].concat()
    } else {
        body
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "the PTY shim's one job is to become the user's shell"
)]
pub fn shim(mut args: impl Iterator<Item = OsString>) -> i32 {
    use std::os::unix::process::CommandExt;
    let Some(shell) = args.next() else {
        eprintln!("usage: yi __pty <shell> [args]");
        return 2;
    };
    let controlling =
        rustix::process::setsid().and_then(|_| rustix::process::ioctl_tiocsctty(std::io::stdin()));
    if let Err(error) = controlling {
        eprintln!("yi __pty: {}", std::io::Error::from(error));
        return 1;
    }
    let error = Command::new(&shell).args(args).exec();
    eprintln!("yi __pty: {}: {error}", shell.to_string_lossy());
    127
}

/// Incident: a clipboard holding `ESC[201~` closed the bracket early and the rest reached the
/// shell as typed keystrokes; ESC never belongs in pasted text.
fn paste_text(text: &str) -> String {
    text.replace('\x1b', "")
}

#[cfg(test)]
mod tests {
    use super::{encode, paste_text};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn a_paste_cannot_close_its_own_bracket() {
        assert_eq!(paste_text("a\x1b[201~; rm x\n"), "a[201~; rm x\n");
    }

    /// Dies with a shell that cannot be driven: an arrow that moves nothing in readline, a
    /// ctrl+c that never interrupts, an alt chord that types a stray letter.
    #[test]
    fn keys_encode_as_xterm_sends_them() {
        let none = KeyModifiers::NONE;
        let cases: [(KeyCode, KeyModifiers, bool, &[u8]); 14] = [
            (KeyCode::Up, none, false, b"\x1b[A"),
            (KeyCode::Up, none, true, b"\x1bOA"),
            (KeyCode::Left, KeyModifiers::CONTROL, true, b"\x1b[1;5D"),
            (KeyCode::Char('c'), KeyModifiers::CONTROL, false, b"\x03"),
            (KeyCode::Char('d'), KeyModifiers::CONTROL, false, b"\x04"),
            (KeyCode::Char('x'), KeyModifiers::ALT, false, b"\x1bx"),
            (KeyCode::Char('é'), none, false, "é".as_bytes()),
            (KeyCode::Backspace, none, false, b"\x7f"),
            (KeyCode::Backspace, KeyModifiers::ALT, false, b"\x1b\x7f"),
            (KeyCode::Enter, none, false, b"\r"),
            (KeyCode::Tab, none, false, b"\t"),
            (KeyCode::Delete, none, false, b"\x1b[3~"),
            (KeyCode::F(1), none, false, b"\x1bOP"),
            (KeyCode::F(5), none, false, b"\x1b[15~"),
        ];
        for (code, modifiers, app_cursor, want) in cases {
            let got = encode(&KeyEvent::new(code, modifiers), app_cursor);
            assert_eq!(got, want, "{code:?} {modifiers:?} app_cursor={app_cursor}");
        }
    }
}
