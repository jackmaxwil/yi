//! This Mac's background services: `launchctl list` joined with the user's property lists.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::thread;
use std::time::{Duration, SystemTime};

use serde_json::Value as Json;
use yd_core::command::table::{Run, Target};
use yd_core::command::tools::service;
use yd_core::feed::{FeedEvent, FeedStatus, SourceCmd};
use yd_core::ids::{CommandId, RowId};
use yd_core::inbox::{Closed, FeedTx};
use yd_core::kind::Kind;
use yd_core::row::{Dot, Row, Value};

use crate::fs::{Mirror, moved};

pub use yd_core::feed::DAEMONS as FEED;

pub const REFRESH: Duration = Duration::from_secs(10);
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_OUTPUT: u64 = 1 << 20;
const MAX_PLISTS: usize = 300;
const TAIL_BYTES: u64 = 64 << 10;
const TAIL_LINES: usize = 20;
const LOGS_BYTES: u64 = 256 << 10;
const LOGS_LINES: usize = 200;
const WIDTH: usize = 120;
/// Signal names by number; an empty one is a signal this table does not name.
const SIGNALS: [&str; 16] = [
    "", "HUP", "INT", "QUIT", "ILL", "", "ABRT", "", "FPE", "KILL", "BUS", "SEGV", "", "PIPE",
    "ALRM", "TERM",
];

/// Why a command did not give output: it could not start, ran too long or too much, or failed.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RunError(pub String);

/// Runs a command by its short name (`launchctl`, `plutil`, `id`) and gives its output.
pub type Runner<'a> = dyn FnMut(&str, &[&str]) -> Result<Vec<u8>, RunError> + 'a;

/// A numeric user id: the `gui/<uid>` domain a user's services live in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Uid(pub u32);

#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub agents: Option<PathBuf>,
    pub project: Option<PathBuf>,
    pub named: Vec<String>,
}

/// Starts a command by absolute path, bounded in time and in output, and kills it past either.
fn spawn(program: &str, args: &[&str]) -> Result<Vec<u8>, RunError> {
    let fail = |why: &str| RunError(format!("{program} {why}"));
    let bin = match program {
        "launchctl" => "/bin/launchctl",
        "plutil" => "/usr/bin/plutil",
        "id" => "/usr/bin/id",
        _ => return Err(fail("is not a command this source runs")),
    };
    #[allow(clippy::disallowed_methods, reason = "the launchctl spawn")]
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| fail("could not be started"))?;
    let stdout = child.stdout.take();
    let (done, got) = channel();
    thread::spawn(move || {
        let mut out = Vec::new();
        let read = stdout.map(|o| o.take(MAX_OUTPUT + 1).read_to_end(&mut out));
        let _ = done.send(read.map(|_| out));
    });
    let out = match got.recv_timeout(TIMEOUT) {
        Ok(Some(out)) if out.len() as u64 <= MAX_OUTPUT => out,
        Ok(None) | Err(RecvTimeoutError::Disconnected) => Vec::new(),
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            let why = format!("ran past {TIMEOUT:?} or wrote more than {MAX_OUTPUT} bytes");
            return Err(fail(&why));
        }
    };
    let status = child.wait().map_err(|_| fail("could not be waited for"))?;
    if status.success() {
        Ok(out)
    } else {
        Err(fail(&format!("exited with {:?}", status.code())))
    }
}

/// A label is passed as one argument inside `gui/<uid>/<label>`: plain characters only.
fn valid_label(label: &str) -> bool {
    let plain = |c: char| c.is_ascii_alphanumeric() || "._-@+:".contains(c);
    !label.is_empty() && label.chars().count() <= 200 && label.chars().all(plain)
}

#[derive(Clone)]
struct Plist {
    label: String,
    program: Option<String>,
    run_at_load: bool,
    errors: Option<PathBuf>,
    output: Option<PathBuf>,
}

/// What `plutil -convert json` printed; none when it is not an object with a label.
fn parse_plist(json: &str) -> Option<Plist> {
    let value: Json = serde_json::from_str(json).ok()?;
    let text = |key: &str| value.get(key).and_then(Json::as_str);
    let first = || value.get("ProgramArguments")?.as_array()?.first()?.as_str();
    Some(Plist {
        label: text("Label")?.to_owned(),
        program: text("Program").or_else(first).map(str::to_owned),
        run_at_load: value.get("RunAtLoad").and_then(Json::as_bool) == Some(true),
        errors: text("StandardErrorPath").map(PathBuf::from),
        output: text("StandardOutPath").map(PathBuf::from),
    })
}

#[derive(Default)]
struct Service {
    pid: Option<u32>,
    status: i32,
    loaded: bool,
    file: Option<PathBuf>,
    plist: Option<Plist>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Known {
    pub loaded: bool,
    pub file: Option<PathBuf>,
    pub log: Option<PathBuf>,
    pub login: bool,
}

fn wanted(label: &str, program: Option<&str>, scope: &Scope) -> bool {
    let inside = match (program, &scope.project) {
        (Some(program), Some(root)) => Path::new(program).starts_with(root),
        _ => false,
    };
    let named = scope.named.iter().any(|named| named == label);
    !label.starts_with("com.apple.")
        && (label.starts_with("com.yi.") || label.starts_with("yi.") || named || inside)
}

/// The labels `launchctl print-disabled` marks disabled.
fn parse_disabled(text: &str) -> BTreeSet<String> {
    let marked = |line: &str| {
        let (label, state) = line.trim().split_once("=>")?;
        let label = label.trim().strip_prefix('"')?.strip_suffix('"')?;
        (state.trim() == "disabled").then(|| label.to_owned())
    };
    text.lines().filter_map(marked).collect()
}

/// The last exit status as a sentence: a signal by name, an exit code as a number.
fn exit_words(status: i32) -> Option<String> {
    let signal = status.unsigned_abs();
    let name = SIGNALS
        .get(usize::try_from(signal).ok()?)
        .filter(|n| !n.is_empty());
    match (status, name) {
        (0, _) => None,
        (n, _) if n > 0 => Some(format!("exit {n}")),
        (_, Some(name)) => Some(format!("killed by signal {name} ({signal})")),
        _ => Some(format!("killed by signal {signal}")),
    }
}

struct Tail {
    lines: Vec<String>,
    /// Lines in the window that was read: all of them unless the file was longer.
    seen: usize,
    cut: bool,
}

impl Tail {
    fn notice(&self, cap: usize, next: &str) -> Option<String> {
        let (kept, seen, more) = (self.lines.len(), self.seen, if self.cut { "+" } else { "" });
        (self.cut || seen > cap)
            .then(|| format!("[… last {kept} of {seen}{more} lines, cap {cap}; {next}]"))
    }
}

/// The last `lines` lines inside the last `bytes` bytes of a file, with control characters
/// dropped; none when it cannot be read.
fn tail(path: &Path, bytes: u64, lines: usize) -> Option<Tail> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(bytes))).ok()?;
    let mut raw = Vec::new();
    file.take(bytes).read_to_end(&mut raw).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let cut = len > bytes;
    let all: Vec<String> = text
        .lines()
        .skip(usize::from(cut))
        .map(|line| line.chars().filter(|c| !c.is_control()).collect())
        .collect();
    let seen = all.len();
    let lines = all.into_iter().skip(seen.saturating_sub(lines)).collect();
    Some(Tail { lines, seen, cut })
}

/// What a failed service says: its exit status and the last line of its error log, and the
/// log's tail as the note.
fn failure(row: Row, status: i32, errors: Option<&Path>) -> Row {
    let why = exit_words(status).unwrap_or_default();
    let kept = errors.and_then(|path| tail(path, TAIL_BYTES, TAIL_LINES));
    let Some(kept) = kept.filter(|t| !t.lines.is_empty()) else {
        return row.text("last_line", why);
    };
    let last = kept.lines.iter().rev().find(|l| !l.trim().is_empty());
    let last: String = last
        .map(|l| l.chars().take(WIDTH).collect())
        .unwrap_or_default();
    let said = if last.is_empty() {
        why
    } else {
        format!("{why}, {last}")
    };
    let next = format!("service.logs gives {LOGS_LINES}");
    let head = kept
        .notice(TAIL_LINES, &next)
        .map_or(String::new(), |n| n + "\n");
    row.text("last_line", said)
        .text("note", head + &kept.lines.join("\n"))
}

fn service_row(label: &str, svc: &Service, login: bool) -> (Row, Known) {
    let state = match (svc.pid, svc.status) {
        (Some(_), _) => Dot::Run,
        (None, 0) => Dot::Stop,
        (None, _) => Dot::Fail,
    };
    let mut row = Row::new(label)
        .cell("state", Value::Dot(state))
        .text("name", label);
    row.dim = state == Dot::Stop;
    if let Some(pid) = svc.pid {
        row = row.text("pid", pid.to_string());
    }
    let logs = svc
        .plist
        .as_ref()
        .map(|p| (p.errors.as_deref(), p.output.as_deref()));
    let (errors, output) = logs.unwrap_or_default();
    if svc.plist.is_some() {
        let words = if login { "starts" } else { "does not start" };
        row = row
            .cell("login", Value::Flag(login))
            .text("at_login", words);
    }
    if state == Dot::Fail {
        row = failure(row, svc.status, errors);
    }
    let log = errors.or(output).map(Path::to_path_buf);
    let mut actions = match state {
        Dot::Run => vec!["Restart", "Stop"],
        Dot::Fail => vec!["Restart"],
        _ => vec!["Start"],
    };
    actions.extend(log.as_ref().map(|_| "Logs"));
    let tags = actions.into_iter().map(str::to_owned).collect();
    let known = Known {
        loaded: svc.loaded,
        file: svc.file.clone(),
        log,
        login,
    };
    (row.cell("actions", Value::Tags(tags)), known)
}

pub struct Reading {
    pub rows: Vec<Row>,
    pub known: BTreeMap<String, Known>,
}

/// What a property list said when it was last read, and the file's size and change time
/// then: a file that has not changed since is not handed to `plutil` again.
type Stamp = (u64, Option<SystemTime>);
#[derive(Default)]
pub struct Plists(BTreeMap<PathBuf, (Stamp, Option<Plist>)>);

fn stamp(file: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(file).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

/// The services `launchctl list` names and the property lists the folder holds, joined by
/// label. Returns how many entries could not be used.
fn gather(
    runner: &mut Runner<'_>,
    scope: &Scope,
    seen: &mut Plists,
) -> Result<(BTreeMap<String, Service>, usize), RunError> {
    let list = runner("launchctl", &["list"])?;
    let (mut all, mut skipped): (BTreeMap<String, Service>, usize) = (BTreeMap::new(), 0);
    for line in String::from_utf8_lossy(&list).lines() {
        let mut cols = line.splitn(3, '\t');
        let (Some(pid), Some(status), Some(label)) = (cols.next(), cols.next(), cols.next()) else {
            continue;
        };
        let Ok(status) = status.parse() else { continue };
        if valid_label(label) {
            let svc = all.entry(label.to_owned()).or_default();
            (svc.pid, svc.status, svc.loaded) = (pid.parse().ok(), status, true);
        } else {
            skipped += 1;
        }
    }
    let dirs = scope
        .agents
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok());
    let mut files: Vec<PathBuf> = dirs
        .flatten()
        .filter_map(|e| Some(e.ok()?.path()))
        .collect();
    files.retain(|path| path.extension().is_some_and(|e| e == "plist"));
    files.sort();
    skipped = skipped.saturating_add(files.len().saturating_sub(MAX_PLISTS));
    files.truncate(MAX_PLISTS);
    seen.0.retain(|file, _| files.contains(file));
    for file in files {
        let now = stamp(&file);
        let kept = seen.0.get(&file).filter(|(then, _)| Some(*then) == now);
        let plist = match kept {
            Some((_, plist)) => plist.clone(),
            None => {
                let args = |p| ["-convert", "json", "-o", "-", p];
                let json = file.to_str().and_then(|p| runner("plutil", &args(p)).ok());
                let plist = json.and_then(|json| parse_plist(&String::from_utf8_lossy(&json)));
                if let Some(now) = now {
                    seen.0.insert(file.clone(), (now, plist.clone()));
                }
                plist
            }
        };
        match plist {
            Some(plist) if valid_label(&plist.label) => {
                let svc = all.entry(plist.label.clone()).or_default();
                (svc.file, svc.plist) = (Some(file), Some(plist));
            }
            _ => skipped = skipped.saturating_add(1),
        }
    }
    Ok((all, skipped))
}

/// Reads the services. The only failure is `launchctl list` itself; a property list that
/// cannot be read is skipped and counted in a row.
pub fn read(
    runner: &mut Runner<'_>,
    scope: &Scope,
    uid: Uid,
    seen: &mut Plists,
) -> Result<Reading, RunError> {
    let (mut all, skipped) = gather(runner, scope, seen)?;
    let domain = format!("gui/{}", uid.0);
    let disabled = runner("launchctl", &["print-disabled", &domain])
        .map(|text| parse_disabled(&String::from_utf8_lossy(&text)))
        .unwrap_or_default();
    let program = |svc: &Service| svc.plist.as_ref().and_then(|p| p.program.clone());
    all.retain(|label, svc| wanted(label, program(svc).as_deref(), scope));
    let (mut rows, mut known) = (Vec::new(), BTreeMap::new());
    for (label, svc) in all.iter() {
        let login = svc.plist.as_ref().is_some_and(|p| p.run_at_load) && !disabled.contains(label);
        let (row, info) = service_row(label, svc, login);
        rows.push(row);
        known.insert(label.clone(), info);
    }
    if skipped > 0 {
        let words = format!("[… {skipped} skipped: not readable as a service or property list]");
        let mut row = Row::new("notice:skipped").text("name", words);
        row.dim = true;
        rows.push(row.cell("state", Value::Dot(Dot::Idle)));
    }
    Ok(Reading { rows, known })
}

/// The `launchctl` arguments for a command on one service, or none when it needs a fact the
/// last reading did not have.
pub fn action_args(
    command: &CommandId,
    uid: Uid,
    label: &str,
    known: &Known,
) -> Option<Vec<String>> {
    let domain = format!("gui/{}", uid.0);
    let target = format!("{domain}/{label}");
    let file = known.file.as_deref().and_then(Path::to_str);
    let args = match (*command, known.loaded, file) {
        (service::START, true, _) => vec!["kickstart", &target],
        (service::START, false, Some(file)) => vec!["bootstrap", &domain, file],
        (service::STOP, ..) => vec!["bootout", &target],
        (service::RESTART, ..) => vec!["kickstart", "-k", &target],
        (service::LOGIN, ..) => vec![if known.login { "disable" } else { "enable" }, &target],
        _ => return None,
    };
    Some(args.into_iter().map(str::to_owned).collect())
}

#[derive(Default)]
struct Source {
    scope: Scope,
    uid: Option<Uid>,
    mirror: Mirror,
    known: BTreeMap<String, Known>,
    logged: BTreeMap<String, usize>,
    plists: Plists,
    /// Whether the services were read at all: a command needs what a reading knows.
    read: bool,
}

impl Source {
    fn uid(&mut self, runner: &mut Runner<'_>) -> Result<Uid, RunError> {
        if let Some(uid) = self.uid {
            return Ok(uid);
        }
        let said = runner("id", &["-u"])?;
        let text = String::from_utf8_lossy(&said).trim().parse();
        let uid = Uid(text.map_err(|_| RunError("id did not give a number".to_owned()))?);
        self.uid = Some(uid);
        Ok(uid)
    }

    fn refresh(&mut self, out: &FeedTx, runner: &mut Runner<'_>) -> Result<(), Closed> {
        let uid = self.uid(runner);
        self.read = true;
        match uid.and_then(|uid| read(runner, &self.scope, uid, &mut self.plists)) {
            Ok(reading) => {
                self.known = reading.known;
                self.mirror.publish(out, reading.rows)
            }
            Err(why) => out.send(FeedEvent::Status(FeedStatus::Error(why.to_string()))),
        }
    }

    fn act(&mut self, out: &FeedTx, runner: &mut Runner<'_>, run: &Run) -> Result<(), Closed> {
        let Target::Thing(address) = &run.target else {
            return Ok(());
        };
        let label = address.id();
        let known = self.known.get(label).cloned();
        let (Some(known), Ok(uid), Kind::Service) = (known, self.uid(runner), address.kind())
        else {
            return Ok(());
        };
        if run.command == service::LOGS {
            return self.logs(out, label, known.log);
        }
        let Some(args) = action_args(&run.command, uid, label, &known) else {
            return Ok(());
        };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        match runner("launchctl", &args) {
            Ok(_) => Ok(()),
            Err(why) => out.send(FeedEvent::Status(FeedStatus::Error(why.to_string()))),
        }
    }

    /// Sends the last [`LOGS_LINES`] lines of the service's log as rows `log:<label>:<n>`,
    /// each line in the `name` cell so the list draws it under its service.
    fn logs(&mut self, out: &FeedTx, label: &str, log: Option<PathBuf>) -> Result<(), Closed> {
        let Some(kept) = log.and_then(|path| tail(&path, LOGS_BYTES, LOGS_LINES)) else {
            let why = format!("{label} names no log file that can be read");
            return out.send(FeedEvent::Status(FeedStatus::Error(why)));
        };
        let row = |n: usize, line: String| Row {
            parent: Some(RowId::new(label)),
            dim: true,
            ..Row::new(format!("log:{label}:{n}"))
                .text("source", label)
                .text("name", line)
        };
        let head = kept
            .notice(LOGS_LINES, "the log file holds the rest")
            .map(|words| row(0, words));
        let lines = kept.lines.into_iter().enumerate();
        let rows: Vec<Row> = head
            .into_iter()
            .chain(lines.map(|(n, l)| row(n.saturating_add(1), l)))
            .collect();
        let before = self.logged.insert(label.to_owned(), rows.len());
        let before = before.unwrap_or(0);
        for n in rows.len()..before {
            out.send(FeedEvent::Remove(RowId::new(format!("log:{label}:{n}"))))?;
        }
        rows.into_iter()
            .try_for_each(|row| out.send(FeedEvent::Upsert(row)))
    }
}

/// Reads when a pane comes to show the feed, every `every` while one does, on `Refresh`, and
/// after any command; with no pane showing it, only a command or `Refresh` makes it read.
pub fn serve(
    out: FeedTx,
    cmds: Receiver<SourceCmd>,
    runner: &mut Runner<'_>,
    scope: Scope,
    every: Duration,
) -> Result<(), Closed> {
    let mut source = Source {
        scope,
        ..Source::default()
    };
    let mut seen = false;
    loop {
        let got = if seen {
            cmds.recv_timeout(every)
        } else {
            cmds.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        let asked = match got {
            Ok(SourceCmd::Run(run)) => {
                if !source.read {
                    source.refresh(&out, runner)?;
                }
                source.act(&out, runner, &run)?;
                true
            }
            Ok(SourceCmd::Root(folder)) => {
                let mut root = source.scope.project.take();
                moved(&mut root, folder);
                source.scope.project = root;
                false
            }
            Ok(SourceCmd::Labels(named)) => {
                source.scope.named = named;
                false
            }
            Ok(SourceCmd::Seen(on)) => {
                if std::mem::replace(&mut seen, on) || !on {
                    continue;
                }
                false
            }
            Ok(SourceCmd::Refresh) => true,
            Err(RecvTimeoutError::Timeout) => false,
            Ok(_) => continue,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        // What a hidden list would show is read when it is next seen, not now.
        if seen || asked {
            source.refresh(&out, runner)?;
        }
    }
}

/// The user's own services: this user's `~/Library/LaunchAgents`, and the project the shown
/// thread works in once it is named.
pub fn run(out: FeedTx, cmds: Receiver<SourceCmd>) -> Result<(), Closed> {
    let agents = std::env::var_os("HOME").map(|home| Path::new(&home).join("Library/LaunchAgents"));
    let scope = Scope {
        agents,
        ..Scope::default()
    };
    serve(out, cmds, &mut spawn, scope, REFRESH)
}

#[cfg(test)]
mod tests;
