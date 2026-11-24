//! One language server's process: started in its own group, its pipes each on a thread,
//! the questions it owes, and how it is stopped.

use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::answers::{Shown, Stats};
use super::{Ask, End, Event, GRACE, Language, MORE_BINS, QUEUE};
use crate::lsp::protocol::{self, ReadError};

/// A file a server has open: the text it was last sent, and that text's version.
#[derive(Debug)]
pub(super) struct Doc {
    pub(super) version: i64,
    pub(super) text: String,
}

pub(super) enum State {
    Starting,
    Ready,
    Failed(String),
}

pub(super) struct Pending {
    pub(super) ask: Ask,
    pub(super) sent: Instant,
}

impl Pending {
    /// When the question runs out of time; none past the clock's range.
    pub(super) fn due(&self) -> Option<Instant> {
        self.sent.checked_add(self.ask.limit())
    }
}

/// A message held until the server has answered `initialize`.
pub(super) enum Out {
    Note(&'static str, Value),
    Call(Ask, Value),
}

pub(super) struct Server {
    pub(super) lang: &'static Language,
    pub(super) serial: u64,
    pub(super) child: Child,
    pub(super) to: SyncSender<Vec<u8>>,
    pub(super) state: State,
    pub(super) stats: Stats,
    pub(super) next: u64,
    pub(super) pending: BTreeMap<u64, Pending>,
    pub(super) held: Vec<Out>,
    pub(super) docs: BTreeMap<String, Doc>,
    /// It has said it is idle once: its workspace is loaded.
    pub(super) idle: bool,
    /// The last line it wrote to stderr, handed over once stderr closes.
    pub(super) said: Receiver<String>,
}

/// The most of one stderr line kept, in characters, and read, in bytes.
const SAID: usize = 240;
const LINE: u64 = 64 << 10;

impl Server {
    pub(super) fn ready(&self) -> bool {
        matches!(self.state, State::Ready)
    }

    pub(super) fn send(&mut self, message: &Value) -> Result<(), String> {
        match self.to.try_send(protocol::frame(message)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                Err(format!("more than {QUEUE} messages waited to be written"))
            }
            Err(TrySendError::Disconnected(_)) => Err("its input closed".to_owned()),
        }
    }

    pub(super) fn hold(&mut self, out: Out) -> Result<(), String> {
        if self.held.len() >= QUEUE {
            return Err(format!("more than {QUEUE} messages waited for it to start"));
        }
        self.held.push(out);
        Ok(())
    }

    pub(super) fn note(&mut self, method: &'static str, params: Value) -> Result<(), String> {
        if !self.ready() {
            return self.hold(Out::Note(method, params));
        }
        self.send(&protocol::notification(method, params))
    }

    /// Sends a question, cancelling any older one it supersedes.
    pub(super) fn call(&mut self, ask: Ask, params: Value) -> Result<(), String> {
        self.held
            .retain(|out| !matches!(out, Out::Call(old, _) if ask.supersedes(old)));
        if !self.ready() && !matches!(ask, Ask::Initialize) {
            return self.hold(Out::Call(ask, params));
        }
        let moot: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, p)| ask.supersedes(&p.ask))
            .map(|(id, _)| *id)
            .collect();
        for id in moot {
            self.pending.remove(&id);
            self.send(&protocol::notification(
                "$/cancelRequest",
                json!({"id": id}),
            ))?;
        }
        let id = self.next;
        self.next = self.next.saturating_add(1);
        self.send(&protocol::request(id, ask.method(), params))?;
        self.pending.insert(
            id,
            Pending {
                ask,
                sent: Instant::now(),
            },
        );
        Ok(())
    }

    pub(super) fn shown(&self) -> Shown<'_> {
        match &self.state {
            State::Starting => Shown::Starting,
            State::Ready => Shown::Ready(&self.stats),
            State::Failed(why) => Shown::Failed(why, &self.stats),
        }
    }
}

/// The program on `PATH`, then in `~/.cargo/bin` and [`MORE_BINS`], past a rustup proxy
/// ([`unproxied`]); never a shell lookup.
pub(super) fn which(program: &str) -> Option<PathBuf> {
    if Path::new(program).is_absolute() {
        return Path::new(program).is_file().then(|| PathBuf::from(program));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let cargo = std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo/bin"));
    let found = std::env::split_paths(&path)
        .chain(cargo)
        .chain(MORE_BINS.iter().map(PathBuf::from))
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|at| at.is_file())?;
    let rustup = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".rustup")));
    Some(match rustup {
        Some(rustup) => unproxied(found, &rustup.join("toolchains")),
        None => found,
    })
}

/// `found` itself, unless it is a rustup proxy: then the program from a toolchain in
/// `toolchains` that has it, since the proxy fails where the folder's toolchain lacks it.
pub(super) fn unproxied(found: PathBuf, toolchains: &Path) -> PathBuf {
    let real = std::fs::canonicalize(&found).ok();
    if real.as_deref().and_then(Path::file_name) != Some("rustup".as_ref()) {
        return found;
    }
    let Some(program) = found.file_name() else {
        return found;
    };
    let listed = std::fs::read_dir(toolchains)
        .into_iter()
        .flatten()
        .flatten();
    let mut have: Vec<PathBuf> = listed
        .map(|toolchain| toolchain.path().join("bin").join(program))
        .filter(|at| at.is_file())
        .collect();
    // ponytail: the last toolchain by name; the folder's own toolchain if versions clash.
    have.sort();
    have.pop().unwrap_or(found)
}

/// Starts a server in `root`, in its own process group, and sends it `initialize`.
#[allow(
    clippy::disallowed_methods,
    reason = "a language server: started by absolute path with an argument list"
)]
pub(super) fn start(
    lang: &'static Language,
    program: &Path,
    root: &Path,
    serial: u64,
    events: &SyncSender<Event>,
) -> io::Result<Server> {
    let mut child = Command::new(program)
        .args(lang.args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        kill(&mut child);
        return Err(io::Error::other("its pipes were not made"));
    };
    let (to, queue) = sync_channel(QUEUE);
    let writer = events.clone();
    thread::spawn(move || write_loop(serial, stdin, &queue, &writer));
    let reader = events.clone();
    thread::spawn(move || read_loop(serial, stdout, &reader));
    let (tell, said) = sync_channel(1);
    thread::spawn(move || tell.send(last_line(stderr)));
    let mut server = Server {
        lang,
        serial,
        child,
        to,
        state: State::Starting,
        stats: Stats::default(),
        next: 1,
        pending: BTreeMap::new(),
        held: Vec::new(),
        docs: BTreeMap::new(),
        idle: false,
        said,
    };
    let uri = protocol::uri_of(root);
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let params = json!({
        "processId": std::process::id(),
        "clientInfo": {"name": "Yi Desktop"},
        "rootUri": uri,
        "workspaceFolders": [{"uri": uri, "name": name}],
        "initializationOptions": (lang.options)(),
        "capabilities": {
            "general": {"positionEncodings": ["utf-16"]},
            "textDocument": {
                "synchronization": {"didSave": false},
                "hover": {"contentFormat": ["markdown", "plaintext"]},
                "definition": {"linkSupport": true},
                "references": {},
                "inlayHint": {},
                "publishDiagnostics": {},
                "codeAction": {"codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["", "quickfix", "refactor", "source"]}}}
            },
            "experimental": {"serverStatusNotification": true}
        }
    });
    if let Err(why) = server.call(Ask::Initialize, params) {
        kill(&mut server.child);
        return Err(io::Error::other(why));
    }
    Ok(server)
}

pub(super) fn read_loop(serial: u64, stdout: ChildStdout, events: &SyncSender<Event>) {
    let mut from = BufReader::new(stdout);
    loop {
        match protocol::read_message(&mut from) {
            Ok(message) => {
                if events.send(Event::Message(serial, message)).is_err() {
                    return;
                }
            }
            Err(ReadError::NotJson) => {}
            Err(error) => {
                let _ = events.send(Event::Ended(serial, End::Read(error)));
                return;
            }
        }
    }
}

/// The last line with words that `stderr` gave before it closed, cut to [`SAID`] characters.
fn last_line(stderr: ChildStderr) -> String {
    let mut from = BufReader::new(stderr);
    let (mut line, mut last) = (Vec::new(), String::new());
    loop {
        line.clear();
        match (&mut from).take(LINE).read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => return last,
            Ok(_) => {
                let words = String::from_utf8_lossy(&line);
                if !words.trim().is_empty() {
                    last = words.trim().chars().take(SAID).collect();
                }
            }
        }
    }
}

/// Writes what the source queues; the server's stdin closes when the queue's sender goes.
pub(super) fn write_loop(
    serial: u64,
    mut stdin: ChildStdin,
    queue: &Receiver<Vec<u8>>,
    events: &SyncSender<Event>,
) {
    for bytes in queue {
        if let Err(error) = stdin.write_all(&bytes).and_then(|()| stdin.flush()) {
            let _ = events.send(Event::Ended(serial, End::Write(error)));
            return;
        }
    }
}

/// Asks each server to shut down and exit, closes its input, and kills the group of any
/// still running after [`GRACE`].
pub(super) fn stop(servers: Vec<Server>) {
    let mut children = Vec::new();
    for server in servers {
        let Server {
            child,
            to,
            state,
            next,
            ..
        } = server;
        if !matches!(state, State::Failed(_)) {
            let _ = to.try_send(protocol::frame(&protocol::request(
                next,
                "shutdown",
                Value::Null,
            )));
            let _ = to.try_send(protocol::frame(&protocol::notification(
                "exit",
                Value::Null,
            )));
        }
        drop(to);
        children.push(child);
    }
    let deadline = Instant::now().checked_add(GRACE);
    for mut child in children {
        let left = deadline.map_or(Duration::ZERO, |at| {
            at.saturating_duration_since(Instant::now())
        });
        if reap(&mut child, left).is_none() {
            kill(&mut child);
        }
    }
}

/// Waits up to `grace` for the child to exit on its own; its status if it did.
pub(super) fn reap(child: &mut Child, grace: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now().checked_add(grace)?;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Kills the child's whole group, then reaps it.
pub(super) fn kill(child: &mut Child) {
    let group = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw);
    if let Some(group) = group {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::Kill);
    }
    let _ = child.kill();
    let _ = child.wait();
}

pub(super) fn exited(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited with code {code}"),
        (None, Some(signal)) => format!("was stopped by signal {signal}"),
        (None, None) => "exited".to_owned(),
    }
}
