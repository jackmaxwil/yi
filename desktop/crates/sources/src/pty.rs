//! A shell on a PTY the app opens, started through `yi __pty`, parsed by `vt100` on its reader
//! thread; lines that scroll off the screen go to the app's own scrollback ring.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use yd_core::feed::{FeedEvent, FeedStatus, SourceCmd, TERMINAL};
use yd_core::ids::FeedId;
use yd_core::inbox::{Closed, FeedTx};
use yd_core::row::Row;

use crate::server::NoYi;

pub const FEED: FeedId = TERMINAL;
/// Lines the scrollback ring keeps; the oldest go first.
pub const SCROLLBACK: usize = 10_000;

/// Bytes parsed between two looks at what scrolled off. Invariant: a piece of at most one screen
/// of bytes scrolls at most a screen by newlines, which `vt100` 0.15 can still show.
const PIECE: usize = 64;
/// Lines `vt100` itself keeps: only enough to count and read one piece's worth.
const PROBE: usize = 2 * PIECE;
/// Bytes parsed under one lock of the parser.
const SLICE: usize = 4096;
const WRITE_QUEUE: usize = 64;
const GRACE: Duration = Duration::from_secs(2);

/// What the reader thread and the terminal surface share; the app builds both from one handle.
#[derive(Clone)]
pub struct Screen {
    pub parser: Arc<Mutex<vt100::Parser>>,
    /// Lines that scrolled off, oldest first, each as `vt100` formats a row.
    pub lines: Arc<Mutex<VecDeque<Vec<u8>>>>,
}

impl Screen {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            parser: Arc::new(Mutex::new(vt100::Parser::new(rows, cols, PROBE))),
            lines: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Parses `bytes` and moves every line that scrolls off into [`Screen::lines`].
    pub fn take(&self, bytes: &[u8]) {
        for slice in bytes.chunks(SLICE) {
            let mut gone = Vec::new();
            {
                let mut parser = lock(&self.parser);
                let rows = usize::from(parser.screen().size().0.max(1));
                for piece in slice.chunks(rows.min(PIECE)) {
                    parse(&mut parser, piece, &mut gone);
                }
            }
            if gone.is_empty() {
                continue;
            }
            let mut lines = lock(&self.lines);
            lines.extend(gone);
            let over = lines.len().saturating_sub(SCROLLBACK);
            lines.drain(..over);
        }
    }
}

/// One piece through the parser. `vt100` raises its scrollback offset by one for each line that
/// leaves while the offset is not zero, which is how the lines are counted.
fn parse(parser: &mut vt100::Parser, piece: &[u8], gone: &mut Vec<Vec<u8>>) {
    let alternate = parser.screen().alternate_screen();
    parser.set_scrollback(1);
    let counting = parser.screen().scrollback() == 1;
    parser.process(piece);
    let left = if alternate != parser.screen().alternate_screen() {
        0
    } else if counting {
        parser.screen().scrollback().saturating_sub(1)
    } else {
        parser.set_scrollback(usize::MAX);
        parser.screen().scrollback()
    };
    let rows = parser.screen().size().0;
    let shown = left.min(usize::from(rows));
    if shown > 0 {
        parser.set_scrollback(shown);
        // Invariant: a width past the screen keeps `vt100` from carrying a wrap into the next row, so
        // each line's bytes move only within their own row.
        gone.extend(parser.screen().rows_formatted(0, u16::MAX).take(shown));
    }
    parser.set_scrollback(0);
}

/// How to start the shell.
#[derive(Debug, Clone, PartialEq)]
pub struct Spawn {
    /// The installed `yi`, whose `__pty` makes the shell a session leader on the PTY.
    pub yi: Result<PathBuf, NoYi>,
    pub shell: PathBuf,
    pub args: Vec<OsString>,
    /// The thread's working folder.
    pub cwd: PathBuf,
    /// The child's `HOME`, when it is not the user's own (tests keep it under `target/`).
    pub home: Option<PathBuf>,
}

impl Spawn {
    /// The installed `yi` and the user's login shell, in `cwd`.
    pub fn installed(cwd: PathBuf) -> Self {
        let shell =
            std::env::var_os("SHELL").map_or_else(|| PathBuf::from("/bin/zsh"), PathBuf::from);
        Self {
            yi: crate::server::installed_yi(),
            shell,
            args: Vec::new(),
            cwd,
            home: None,
        }
    }
}

/// A shell that could not start, or whose PTY failed.
#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    #[error("the PTY could not be opened: {0}")]
    Open(#[source] std::io::Error),
    #[error(transparent)]
    NoYi(#[from] NoYi),
    #[error("the folder {} is gone", .0.display())]
    Gone(PathBuf),
    #[error("{shell} did not start: {source}")]
    Start {
        shell: String,
        #[source]
        source: std::io::Error,
    },
}

/// The registry's row: a shell in the user's home with a screen nobody else reads. The app
/// starts a tab's shell with [`serve`] and the tab's own [`Screen`].
pub fn run(out: FeedTx, cmds: Receiver<SourceCmd>) -> Result<(), Closed> {
    let cwd = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    serve(&Screen::new(24, 80), &Spawn::installed(cwd), out, cmds)
}

/// One shell for one feed instance: output into `screen`, input and size from `cmds`. On
/// `SourceCmd::Close` or when `cmds` closes the shell's group is hung up, then killed after a grace.
pub fn serve(
    screen: &Screen,
    spawn: &Spawn,
    out: FeedTx,
    cmds: Receiver<SourceCmd>,
) -> Result<(), Closed> {
    let mut shell = match Shell::start(screen, spawn, out.clone()) {
        Ok(shell) => shell,
        Err(error) => return out.send(FeedEvent::Status(FeedStatus::Error(error.to_string()))),
    };
    out.send(FeedEvent::Status(FeedStatus::Live))?;
    for cmd in cmds {
        let fault = match cmd {
            SourceCmd::Input(bytes) => shell.send(bytes),
            SourceCmd::Close => break,
            SourceCmd::Resize { cols, rows } => shell.resize(screen, cols, rows),
            // A shell has no folder to be pointed at, no file to read and no hunk to revert.
            SourceCmd::Run(_)
            | SourceCmd::Refresh
            | SourceCmd::Root(_)
            | SourceCmd::Read(_)
            | SourceCmd::Revert { .. }
            | SourceCmd::Screen(_)
            | SourceCmd::Seen(_)
            | SourceCmd::Labels(_)
            | SourceCmd::Readable(_) => Ok(()),
        };
        if let Err(error) = fault {
            out.send(FeedEvent::Status(FeedStatus::Error(error.to_string())))?;
        }
    }
    shell.stop();
    Ok(())
}

struct Shell {
    master: File,
    writes: SyncSender<Vec<u8>>,
    /// The child until the reader reaps it: whoever holds the lock decides, so a reaped pid is
    /// never signalled.
    child: Arc<Mutex<Option<Child>>>,
    done: Receiver<()>,
    size: (u16, u16),
}

impl Shell {
    #[allow(
        clippy::disallowed_methods,
        reason = "the terminal's one spawn: the shell through Yi's PTY shim by absolute path"
    )]
    fn start(screen: &Screen, spawn: &Spawn, out: FeedTx) -> Result<Self, PtyError> {
        use rustix::pty::OpenptFlags;
        if !spawn.cwd.is_dir() {
            return Err(PtyError::Gone(spawn.cwd.clone()));
        }
        use std::os::unix::ffi::OsStringExt;
        let opened = || -> std::io::Result<(File, File)> {
            let master = rustix::pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
            rustix::io::fcntl_setfd(&master, rustix::io::FdFlags::CLOEXEC)?;
            rustix::pty::grantpt(&master)?;
            rustix::pty::unlockpt(&master)?;
            let name = OsString::from_vec(rustix::pty::ptsname(&master, Vec::new())?.into_bytes());
            let slave = File::options().read(true).write(true).open(name)?;
            Ok((File::from(master), slave))
        };
        let (master, slave) = opened().map_err(PtyError::Open)?;
        let (rows, cols) = lock(&screen.parser).screen().size();
        rustix::termios::tcsetwinsize(&master, winsize(rows, cols))
            .map_err(|e| PtyError::Open(e.into()))?;
        let start = |source| PtyError::Start {
            shell: spawn.shell.display().to_string(),
            source,
        };
        let yi = spawn.yi.clone()?;
        if !yi.is_absolute() {
            let relative = std::io::Error::other("yi is not at an absolute path");
            return Err(start(relative));
        }
        let mut command = Command::new(&yi);
        // Incident: a GUI app starts with no LANG, and zsh then echoed typed 日本 as <0097> bytes.
        if std::env::var_os("LANG").is_none() {
            command.env("LANG", "en_US.UTF-8");
        }
        if let Some(home) = &spawn.home {
            command.env("HOME", home);
        }
        let stdio = || slave.try_clone().map(Stdio::from).map_err(start);
        let child = command
            .arg("__pty")
            .arg(&spawn.shell)
            .args(&spawn.args)
            .current_dir(&spawn.cwd)
            .env("TERM", "xterm-256color")
            .stdin(stdio()?)
            .stdout(stdio()?)
            .stderr(stdio()?)
            .spawn()
            .map_err(start)?;
        drop(slave);
        let reader = master.try_clone().map_err(PtyError::Open)?;
        let writer = master.try_clone().map_err(PtyError::Open)?;
        let child = Arc::new(Mutex::new(Some(child)));
        let (finished, done) = mpsc::channel();
        let (shared, slot) = (screen.clone(), Arc::clone(&child));
        std::thread::spawn(move || {
            read(reader, &shared, &out);
            let reaped = lock(&slot).take().map(|mut child| child.wait());
            let exit = match reaped {
                Some(Ok(status)) => status
                    .code()
                    .map_or_else(|| "signal".to_owned(), |c| c.to_string()),
                Some(Err(_)) | None => "unknown".to_owned(),
            };
            let _ = out.send(FeedEvent::Upsert(Row::new("shell").text("exit", exit)));
            let _ = out.send(FeedEvent::Status(FeedStatus::Offline));
            let _ = finished.send(());
        });
        let (writes, pending) = mpsc::sync_channel::<Vec<u8>>(WRITE_QUEUE);
        std::thread::spawn(move || write(writer, &pending));
        Ok(Self {
            master,
            writes,
            child,
            done,
            size: (rows, cols),
        })
    }

    /// Invariant: this thread never writes the master itself, and the queue behind it is capped.
    fn send(&self, bytes: Vec<u8>) -> std::io::Result<()> {
        self.writes.try_send(bytes).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => std::io::Error::new(
                ErrorKind::WouldBlock,
                "input dropped: the shell is not reading",
            ),
            mpsc::TrySendError::Disconnected(_) => {
                std::io::Error::new(ErrorKind::BrokenPipe, "the shell's input is closed")
            }
        })
    }

    /// Tells the shell its new size (SIGWINCH through the PTY) and sizes the parser to match.
    fn resize(&mut self, screen: &Screen, cols: u16, rows: u16) -> std::io::Result<()> {
        if rows == 0 || cols == 0 || self.size == (rows, cols) {
            return Ok(());
        }
        self.size = (rows, cols);
        {
            let mut parser = lock(&screen.parser);
            if parser.screen().size() != (rows, cols) {
                parser.set_size(rows, cols);
            }
        }
        Ok(rustix::termios::tcsetwinsize(
            &self.master,
            winsize(rows, cols),
        )?)
    }

    /// Hangs up the shell's group, then kills it if it is still there after [`GRACE`].
    fn stop(self) {
        let signal = |sig| {
            if let Some(child) = lock(&self.child).as_ref() {
                let group = rustix::process::Pid::from_child(child);
                let _ = rustix::process::kill_process_group(group, sig);
            }
        };
        signal(rustix::process::Signal::Hup);
        if self.done.recv_timeout(GRACE).is_err() {
            signal(rustix::process::Signal::Kill);
        }
    }
}

fn read(mut reader: File, screen: &Screen, out: &FeedTx) {
    let mut buf = vec![0_u8; 64 * 1024];
    loop {
        let got = match reader.read(&mut buf) {
            Ok(0) => return,
            Ok(got) => got,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return,
        };
        screen.take(buf.get(..got).unwrap_or_default());
        // Invariant: one event per read is the surface's wake; the parser holds what it shows.
        if out.send(FeedEvent::Status(FeedStatus::Live)).is_err() {
            return;
        }
    }
}

fn write(mut writer: File, pending: &Receiver<Vec<u8>>) {
    for bytes in pending {
        if writer.write_all(&bytes).is_err() {
            return;
        }
    }
}

fn winsize(rows: u16, cols: u16) -> rustix::termios::Winsize {
    rustix::termios::Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Sender};
    use std::time::{Duration, Instant};

    use yd_core::feed::{FeedStatus, SourceCmd};
    use yd_core::inbox::{Inbox, inbox};
    use yd_core::model::Model;

    use super::{FEED, SCROLLBACK, Screen, Spawn, lock, serve};

    struct Running {
        screen: Screen,
        cmds: Sender<SourceCmd>,
        inbox: Inbox,
        model: Model,
        thread: std::thread::JoinHandle<()>,
    }

    /// The child's HOME and working folder: under the lane's `target/`, never the user's own.
    fn home() -> Result<PathBuf, Box<dyn Error>> {
        let exe = std::env::current_exe()?;
        let target = exe.ancestors().find(|p| p.ends_with("target"));
        let home = target.ok_or("no target folder")?.join("pty-home");
        std::fs::create_dir_all(&home)?;
        Ok(home)
    }

    fn start(rows: u16, cols: u16) -> Result<Option<Running>, Box<dyn Error>> {
        let home = home()?;
        let mut spawn = Spawn::installed(home.clone());
        let yi = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".local/bin/yi");
        if !yi.exists() {
            eprintln!("skipped: no yi at {}", yi.display());
            return Ok(None);
        }
        spawn.yi = Ok(yi);
        spawn.shell = PathBuf::from("/bin/zsh");
        spawn.args = vec!["-f".into()];
        spawn.home = Some(home);
        let screen = Screen::new(rows, cols);
        let (sender, inbox) = inbox(|| {});
        let (cmds, rx) = mpsc::channel();
        let (shared, out) = (screen.clone(), sender.feed(FEED));
        let thread = std::thread::spawn(move || {
            let _ = serve(&shared, &spawn, out, rx);
        });
        let mut running = Running {
            screen,
            cmds,
            inbox,
            model: Model::default(),
            thread,
        };
        running.wait(|s| s.contains('%'))?;
        Ok(Some(running))
    }

    impl Running {
        fn shown(&self) -> String {
            lock(&self.screen.parser).screen().contents()
        }

        fn wait(&mut self, until: impl Fn(&str) -> bool) -> Result<String, Box<dyn Error>> {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                self.inbox.drain(&mut self.model);
                let shown = self.shown();
                if until(&shown) {
                    return Ok(shown);
                }
                if Instant::now() > deadline {
                    return Err(format!("timed out; screen:\n{shown}").into());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }

        fn typed(&self, line: &str) -> Result<(), Box<dyn Error>> {
            let keys = format!("{line}\r").into_bytes();
            Ok(self.cmds.send(SourceCmd::Input(keys))?)
        }
    }

    /// Dogfood: a restored tab whose checkout was removed read "/bin/zsh did not start: No such
    /// file or directory", blaming the shell for a folder that is gone.
    #[test]
    fn a_shell_in_a_folder_that_is_gone_says_the_folder_is_gone() -> Result<(), Box<dyn Error>> {
        let gone = home()?.join("removed-checkout");
        let _ = std::fs::remove_dir_all(&gone);
        let spawn = Spawn::installed(gone.clone());
        let (sender, _inbox) = inbox(|| {});
        let said = match super::Shell::start(&Screen::new(24, 80), &spawn, sender.feed(FEED)) {
            Ok(_) => return Err("a shell started in a folder that does not exist".into()),
            Err(error) => error.to_string(),
        };
        assert_eq!(said, format!("the folder {} is gone", gone.display()));
        Ok(())
    }

    /// Dies with a terminal that cannot run a command: input never reaches the shell or its
    /// answer never reaches the screen.
    #[test]
    fn a_typed_command_answers_on_the_screen() -> Result<(), Box<dyn Error>> {
        let Some(mut shell) = start(24, 80)? else {
            return Ok(());
        };
        shell.typed("echo hi-$((6*7)) $TERM")?;
        shell.wait(|s| s.lines().any(|l| l == "hi-42 xterm-256color"))?;
        Ok(())
    }

    /// Dies with Ctrl-C that never interrupts: the shell sits in `sleep` and the next line waits.
    #[test]
    fn ctrl_c_interrupts_the_running_command() -> Result<(), Box<dyn Error>> {
        let Some(mut shell) = start(24, 80)? else {
            return Ok(());
        };
        shell.typed("sleep 30")?;
        std::thread::sleep(Duration::from_millis(300));
        shell.cmds.send(SourceCmd::Input(vec![0x03]))?;
        shell.typed("echo after")?;
        shell.wait(|s| s.lines().any(|l| l == "after"))?;
        Ok(())
    }

    /// Dies with a shell that keeps its first size: `stty size` answers what the surface asked.
    #[test]
    fn the_shell_learns_its_new_size() -> Result<(), Box<dyn Error>> {
        let Some(mut shell) = start(24, 80)? else {
            return Ok(());
        };
        let size = SourceCmd::Resize { cols: 50, rows: 12 };
        shell.cmds.send(size)?;
        shell.typed("stty size")?;
        shell.wait(|s| s.lines().any(|l| l == "12 50"))?;
        assert_eq!(lock(&shell.screen.parser).screen().size(), (12, 50));
        Ok(())
    }

    /// Dies with scrollback that stops at one screen, loses lines, or grows without bound.
    #[test]
    fn lines_that_scroll_off_are_kept_up_to_the_cap() -> Result<(), Box<dyn Error>> {
        let Some(mut shell) = start(10, 40)? else {
            return Ok(());
        };
        shell.typed("seq 1 12000")?;
        shell.wait(|s| s.lines().any(|l| l == "12000"))?;
        let lines = lock(&shell.screen.lines).clone();
        assert_eq!(lines.len(), SCROLLBACK);
        let text = |n: usize| -> Option<String> {
            let mut row = vt100::Parser::new(1, 40, 0);
            row.process(lines.get(n)?);
            Some(row.screen().contents())
        };
        let last = text(SCROLLBACK - 1).ok_or("no last line")?;
        let first: u32 = text(0).ok_or("no first line")?.parse()?;
        let newest: u32 = last.parse()?;
        assert_eq!(
            newest - first,
            u32::try_from(SCROLLBACK)? - 1,
            "the kept lines run on without a gap"
        );
        Ok(())
    }

    /// Dies with a shell left running after its tab closes.
    #[test]
    fn closing_the_feed_ends_the_shell() -> Result<(), Box<dyn Error>> {
        let Some(mut shell) = start(24, 80)? else {
            return Ok(());
        };
        shell.typed("echo $$")?;
        let shown = shell.wait(|s| s.lines().filter(|l| l.parse::<u32>().is_ok()).count() > 0)?;
        let pid: i32 = shown.lines().find_map(|l| l.parse().ok()).ok_or("no pid")?;
        let Running {
            cmds,
            thread,
            inbox,
            mut model,
            ..
        } = shell;
        drop(cmds);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !thread.is_finished() && Instant::now() < deadline {
            inbox.drain(&mut model);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(thread.is_finished(), "serve returned once the feed closed");
        let pid = rustix::process::Pid::from_raw(pid).ok_or("pid 0")?;
        let gone = (0..100).any(|_| {
            inbox.drain(&mut model);
            std::thread::sleep(Duration::from_millis(20));
            rustix::process::test_kill_process(pid).is_err()
        });
        assert!(gone, "the shell {pid:?} is still running");
        let table = model.tables.get(FEED).ok_or("no table")?;
        assert_eq!(table.status(), &FeedStatus::Offline);
        Ok(())
    }

    /// Dies on the `vt100` 0.15 trap or a miscount: a burst of lines in one write.
    #[test]
    fn a_burst_keeps_every_line_in_order() -> Result<(), Box<dyn Error>> {
        let screen = Screen::new(5, 20);
        let burst: String = (1..=500).map(|n| format!("{n}\r\n")).collect();
        screen.take(burst.as_bytes());
        screen.take(b"\x1b[3S");
        let lines = lock(&screen.lines);
        let texts: Vec<String> = lines
            .iter()
            .map(|line| {
                let mut row = vt100::Parser::new(1, 20, 0);
                row.process(line);
                row.screen().contents()
            })
            .collect();
        let want: Vec<String> = (1..=496).map(|n| n.to_string()).collect();
        assert_eq!(texts.get(..496), Some(want.as_slice()));
        assert_eq!(texts.len(), 499, "three more scrolled off by CSI S");
        Ok(())
    }
}
