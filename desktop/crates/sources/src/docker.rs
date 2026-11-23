//! Docker containers, read with the `docker` command and joined into rows by compose project.

use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value as Json};
use yd_core::address::uri::Address;
use yd_core::command::table::{Run, Target};
use yd_core::command::tools::container as cmd;
use yd_core::feed::{FeedEvent, FeedStatus, SourceCmd};
use yd_core::ids::CommandId;
use yd_core::inbox::{Closed, FeedTx};
use yd_core::kind::Kind;
use yd_core::row::{Dot, Row, Value};

pub use yd_core::feed::CONTAINERS as FEED;

/// Where the `docker` binary may be; the first that exists is used, never a shell lookup.
const BINS: &[&str] = &[
    "/usr/local/bin/docker",
    "/opt/homebrew/bin/docker",
    "/Applications/Docker.app/Contents/Resources/bin/docker",
];
/// How often the containers are read again while the source runs.
pub const POLL: Duration = Duration::from_secs(5);
/// The longest one `docker` run may take before it and its group are killed.
const DEADLINE: Duration = Duration::from_secs(10);
/// The most one run may write to either stream.
const MAX_OUTPUT: usize = 8 << 20;
/// Containers listed; the rest are counted in the status.
pub const MAX_CONTAINERS: usize = 500;
/// Lines of a container's log sent by `container.logs` (`docker logs --tail`).
pub const LOG_LINES: usize = 200;
/// Characters of one log line kept.
const LINE_CHARS: usize = 2_000;
/// The group of a container that no compose project owns.
pub const OTHER: &str = "other";
const PROJECT: &str = "com.docker.compose.project=";

/// Why a `docker` run gave nothing; each message is the sentence the pane shows.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RunError {
    #[error("Docker is not installed on this Mac")]
    Missing,
    #[error("Docker is not running: start Docker Desktop and this list fills in")]
    Daemon,
    #[error("Docker refused permission: {0}")]
    Denied(String),
    #[error("docker ran past {DEADLINE:?} and was stopped")]
    Timeout,
    #[error("docker wrote more than {MAX_OUTPUT} bytes")]
    TooLong,
    #[error("docker could not be started: {0}")]
    Start(String),
    #[error("docker said: {0}")]
    Failed(String),
}

/// What one finished `docker` run wrote.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ran {
    pub ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Runs `docker <args>`; a test hands in recorded output and reads the arguments it was given.
pub type Runner<'a> = &'a dyn Fn(&[String]) -> Result<Ran, RunError>;

/// Runs the real `docker`, killed with its group past [`DEADLINE`].
pub fn spawn(args: &[String]) -> Result<Ran, RunError> {
    let bin = BINS
        .iter()
        .find(|bin| Path::new(bin).exists())
        .ok_or(RunError::Missing)?;
    #[allow(clippy::disallowed_methods, reason = "the docker spawn")]
    let mut child = Command::new(bin)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|why| RunError::Start(why.to_string()))?;
    let (out, err) = (pipe(child.stdout.take()), pipe(child.stderr.take()));
    let end = Instant::now().checked_add(DEADLINE);
    let mut streams = Vec::new();
    for got in [out, err] {
        let left = end.map_or(DEADLINE, |end| {
            end.saturating_duration_since(Instant::now())
        });
        match got.recv_timeout(left) {
            Ok(bytes) if bytes.len() > MAX_OUTPUT => {
                return Err(stop(&mut child, RunError::TooLong));
            }
            Ok(bytes) => streams.push(bytes),
            Err(RecvTimeoutError::Timeout) => return Err(stop(&mut child, RunError::Timeout)),
            Err(RecvTimeoutError::Disconnected) => streams.push(Vec::new()),
        }
    }
    let status = child
        .wait()
        .map_err(|why| RunError::Start(why.to_string()))?;
    let stderr = streams.pop().unwrap_or_default();
    let stdout = streams.pop().unwrap_or_default();
    Ok(Ran {
        ok: status.success(),
        stdout,
        stderr,
    })
}

/// Reads one stream of the child on its own thread, so neither stream can fill and stall it.
fn pipe(stream: Option<impl Read + Send + 'static>) -> Receiver<Vec<u8>> {
    let (done, got) = channel();
    thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(stream) = stream {
            let _ = stream.take(MAX_OUTPUT as u64 + 1).read_to_end(&mut out);
        }
        let _ = done.send(out);
    });
    got
}

/// Kills `child`'s whole group and reaps it.
fn stop(child: &mut Child, why: RunError) -> RunError {
    let group = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw);
    if let Some(group) = group {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::Kill);
    }
    let _ = child.kill();
    let _ = child.wait();
    why
}

/// The run's output, or the reason it failed told from what `docker` wrote to stderr.
fn ask(runner: Runner<'_>, args: &[&str]) -> Result<Ran, RunError> {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let ran = runner(&args)?;
    if ran.ok {
        return Ok(ran);
    }
    let said = String::from_utf8_lossy(&ran.stderr);
    let first = said.lines().find(|line| !line.trim().is_empty());
    let first = first.unwrap_or("it gave no reason").trim();
    let low = first.to_lowercase();
    Err(
        if low.contains("cannot connect to the docker daemon")
            || low.contains("is the docker daemon running")
        {
            RunError::Daemon
        } else if low.contains("permission denied") {
            RunError::Denied(first.chars().take(160).collect())
        } else {
            RunError::Failed(first.chars().take(160).collect())
        },
    )
}

/// A Docker container name: the only text this source puts in an argument or a shell line.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
}

/// The container a command is aimed at, when its address names one.
pub fn named(target: &Target) -> Option<&str> {
    match target {
        Target::Thing(address) if address.kind() == Kind::Container => {
            Some(address.id()).filter(|name| valid_name(name))
        }
        _ => None,
    }
}

/// The address of a container, for the command that acts on it.
pub fn address(name: &str) -> Option<Address> {
    Address::new(Kind::Container, name).ok()
}

/// The arguments of a command that changes a container.
fn change(command: CommandId, name: &str) -> Option<Vec<&str>> {
    let verb = match command {
        cmd::RESTART => "restart",
        cmd::STOP => "stop",
        cmd::START => "start",
        cmd::REMOVE => "rm",
        _ => return None,
    };
    Some(vec![verb, name])
}

/// One reading: the rows, and a sentence when part of the output was left out.
#[derive(Debug, Default, PartialEq)]
pub struct Reading {
    pub rows: Vec<Row>,
    pub note: Option<String>,
}

fn objects(out: &[u8]) -> (Vec<Map<String, Json>>, usize) {
    let text = String::from_utf8_lossy(out);
    let mut skipped = 0;
    let mut found = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str::<Json>(line) {
            Ok(Json::Object(map)) => found.push(map),
            _ => skipped += 1,
        }
    }
    (found, skipped)
}

fn field<'a>(map: &'a Map<String, Json>, name: &str) -> &'a str {
    map.get(name).and_then(Json::as_str).unwrap_or("").trim()
}

/// Joins `docker ps -a` lines to `docker stats` lines by container id and groups them.
pub fn read(ps: &[u8], stats: Option<&[u8]>) -> Reading {
    let (listed, mut skipped) = objects(ps);
    let (measured, bad) = stats.map_or((Vec::new(), 0), objects);
    skipped += bad;
    let mut boxes = Vec::new();
    for map in &listed {
        let name = field(map, "Names").split(',').next().unwrap_or("");
        let name = name.trim().trim_start_matches('/');
        if valid_name(name) {
            boxes.push((name, map));
        } else {
            skipped += 1;
        }
    }
    let total = boxes.len();
    boxes.truncate(MAX_CONTAINERS);
    let mut sorted: Vec<(bool, String, Row, bool)> = boxes
        .into_iter()
        .map(|(name, map)| {
            let id = field(map, "ID");
            let usage = measured.iter().find(|m| {
                let other = field(m, "ID");
                !id.is_empty()
                    && !other.is_empty()
                    && (id.starts_with(other) || other.starts_with(id))
            });
            let (row, running) = row(name, map, usage);
            let group = row.group.clone().unwrap_or_default();
            (group == OTHER, group, row, running)
        })
        .collect();
    sorted.sort_by(|a, b| (a.0, &a.1, &a.2.id).cmp(&(b.0, &b.1, &b.2.id)));
    let mut size: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for (_, group, _, running) in &sorted {
        let count = size.entry(group.clone()).or_default();
        count.0 = count.0.saturating_add(1);
        count.1 = count.1.saturating_add(usize::from(*running));
    }
    let mut last = String::new();
    let mut rows = Vec::new();
    for (other, group, row, _) in sorted {
        let mut row = row;
        if group != last {
            let (all, up) = size.get(&group).copied().unwrap_or_default();
            let (count, word) = if up > 0 {
                (up, "running")
            } else {
                (all, "stopped")
            };
            let head = if other { "" } else { "compose · " };
            row = row.text("group.summary", format!("{head}{count} {word}"));
            last = group;
        }
        rows.push(row);
    }
    let mut notes = Vec::new();
    if total > MAX_CONTAINERS {
        notes.push(format!(
            "showing {MAX_CONTAINERS} of {total} containers (cap {MAX_CONTAINERS}); `docker ps -a` lists them all"
        ));
    }
    if skipped > 0 {
        notes.push(format!(
            "{skipped} lines of docker's output were not understood and skipped"
        ));
    }
    Reading {
        rows,
        note: (!notes.is_empty()).then(|| notes.join("; ")),
    }
}

fn row(name: &str, map: &Map<String, Json>, usage: Option<&Map<String, Json>>) -> (Row, bool) {
    let (state, status) = (field(map, "State"), field(map, "Status"));
    let code = status
        .strip_prefix("Exited (")
        .and_then(|rest| rest.split_once(')'))
        .and_then(|(code, _)| code.parse::<i32>().ok());
    let dot = match state {
        "running" => Dot::Run,
        "paused" => Dot::Pause,
        "restarting" => Dot::Work,
        "exited" if code.is_some_and(|code| code != 0) => Dot::Fail,
        "exited" => Dot::Stop,
        "dead" => Dot::Fail,
        _ => Dot::Idle,
    };
    let running = matches!(state, "running" | "paused" | "restarting");
    let mut row = Row::new(name)
        .cell("state", Value::Dot(dot))
        .text("name", name);
    row.dim = !running;
    row.group = Some(
        field(map, "Labels")
            .split(',')
            .find_map(|label| label.trim().strip_prefix(PROJECT))
            .filter(|project| !project.is_empty())
            .unwrap_or(OTHER)
            .to_owned(),
    );
    if !field(map, "Image").is_empty() {
        row = row.text("image", field(map, "Image"));
    }
    if let Some(usage) = usage {
        if let Some(cpu) = percent(field(usage, "CPUPerc")) {
            row = row.cell("cpu", cpu);
        }
        if let Some(mem) = field(usage, "MemUsage")
            .split('/')
            .next()
            .and_then(megabytes)
        {
            row = row.text("mem", mem.clone()).text("memory", mem);
        }
    }
    if let Some(ports) = ports(field(map, "Ports")) {
        row = row.text("ports", ports);
    }
    let up = if running {
        uptime(status)
    } else {
        code.map(|code| format!("exit {code}"))
    };
    if let Some(up) = up {
        row = row.text("up", up);
    }
    let health = [
        ("(healthy)", "passing"),
        ("(unhealthy)", "failing"),
        ("(health: starting)", "starting"),
    ];
    if let Some((_, word)) = health.iter().find(|(tag, _)| status.contains(tag)) {
        row = row.text("health", *word);
    }
    let tags: &[&str] = if running {
        &["Logs", "Shell", "Restart", "Stop", "Attach"]
    } else {
        &["Start", "Logs", "Remove", "Attach"]
    };
    let tags = tags.iter().map(|tag| (*tag).to_owned()).collect();
    (row.cell("actions", Value::Tags(tags)), running)
}

fn percent(text: &str) -> Option<Value> {
    let value: f32 = text.trim_end_matches('%').trim().parse().ok()?;
    value.is_finite().then(|| Value::Meter {
        ratio: (value / 100.0).clamp(0.0, 1.0),
        text: format!("{value:.0}%"),
    })
}

/// "212MiB" as "212 MB", "1.9GiB" as "1.9 GB": the mockup's units.
fn megabytes(text: &str) -> Option<String> {
    let text = text.trim();
    let at = text.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (number, unit) = text.split_at_checked(at)?;
    let number: f64 = number.parse().ok()?;
    let scale = match unit.trim().to_lowercase().as_str() {
        "b" => 1.0,
        "kb" | "kib" => 1024.0,
        "mb" | "mib" => 1024.0 * 1024.0,
        "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let bytes = number * scale;
    Some(if bytes >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB", bytes / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024.0 * 1024.0 {
        format!("{:.0} MB", bytes / (1024.0 * 1024.0))
    } else if bytes >= 1024.0 {
        format!("{:.0} kB", bytes / 1024.0)
    } else {
        format!("{bytes:.0} B")
    })
}

/// "0.0.0.0:8080->8080/tcp, [::]:8080->8080/tcp, 5432/tcp" as "8080→8080, 5432".
fn ports(text: &str) -> Option<String> {
    let mut seen: Vec<String> = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let part = part.rsplit_once('/').map_or(part, |(port, _)| port);
        let shown = match part.split_once("->") {
            Some((host, inner)) => {
                let host = host.rsplit_once(':').map_or(host, |(_, port)| port);
                format!("{host}→{inner}")
            }
            None => part.to_owned(),
        };
        if !seen.contains(&shown) {
            seen.push(shown);
        }
    }
    (!seen.is_empty()).then(|| seen.join(", "))
}

/// "Up 14 minutes (healthy)" as "14m"; "Up About an hour" as "1h".
fn uptime(status: &str) -> Option<String> {
    let mut words = status.strip_prefix("Up ")?.split_whitespace();
    let (count, unit) = match words.next()? {
        "Less" => return Some("0s".to_owned()),
        "About" => (1, words.nth(1)?),
        number => (number.parse::<u32>().ok()?, words.next()?),
    };
    let suffix = ["second", "minute", "hour", "day", "week", "month", "year"]
        .iter()
        .zip(["s", "m", "h", "d", "w", "mo", "y"])
        .find(|(long, _)| unit.starts_with(*long))?
        .1;
    Some(format!("{count}{suffix}"))
}

/// The last lines of a container's log as rows hung under it, with a row saying so when the
/// cap cut older ones.
fn log_rows(name: &str, out: &str) -> Vec<Row> {
    let lines: Vec<&str> = out.lines().collect();
    let mut rows = Vec::new();
    if lines.len() >= LOG_LINES {
        let note = format!(
            "[… last {LOG_LINES} lines of {name} (cap --tail {LOG_LINES}); `docker logs {name}` gives the rest]"
        );
        rows.push(log_row(name, 0, note));
    }
    for (n, line) in lines.iter().enumerate() {
        let text: String = line.chars().take(LINE_CHARS).collect();
        let cut = line.chars().count().saturating_sub(LINE_CHARS);
        let text = if cut > 0 {
            format!("{text} […+{cut} characters, cap {LINE_CHARS}]")
        } else {
            text
        };
        rows.push(log_row(name, n.saturating_add(1), text));
    }
    rows
}

fn log_row(name: &str, n: usize, text: String) -> Row {
    let mut row = Row::new(format!("log:{name}:{n}")).text("last_line", text);
    row.parent = Some(yd_core::ids::RowId::new(name));
    row.dim = true;
    row
}

/// What the source has told the app, so only a change goes out.
#[derive(Debug)]
struct Sent {
    rows: Vec<Row>,
    status: FeedStatus,
    logs: BTreeMap<String, Vec<Row>>,
    refused: Option<String>,
}

impl Sent {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            status: FeedStatus::Loading,
            logs: BTreeMap::new(),
            refused: None,
        }
    }

    fn status(&mut self, out: &FeedTx, status: FeedStatus) -> Result<(), Closed> {
        if self.status != status {
            out.send(FeedEvent::Status(status.clone()))?;
            self.status = status;
        }
        Ok(())
    }

    /// Sends what differs from the last reading: a reset when the rows or their order moved,
    /// else the rows that changed.
    fn publish(&mut self, out: &FeedTx, reading: Reading) -> Result<(), Closed> {
        let mut rows = reading.rows;
        let live: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        self.logs.retain(|name, _| live.contains(&name.as_str()));
        rows.extend(self.logs.values().flatten().cloned());
        let same = rows.len() == self.rows.len()
            && rows
                .iter()
                .zip(&self.rows)
                .all(|(new, old)| new.id == old.id);
        let mut sent = false;
        if same {
            for (new, old) in rows.iter().zip(&self.rows) {
                if new != old {
                    out.send(FeedEvent::Upsert(new.clone()))?;
                    sent = true;
                }
            }
        } else {
            out.send(FeedEvent::Reset(rows.clone()))?;
            sent = true;
        }
        if sent {
            self.status = if rows.is_empty() {
                FeedStatus::Empty
            } else {
                FeedStatus::Live
            };
        }
        self.rows = rows;
        let want = match self.refused.take().or(reading.note) {
            Some(note) => FeedStatus::Error(note),
            None if self.rows.is_empty() => FeedStatus::Empty,
            None => FeedStatus::Live,
        };
        self.status(out, want)
    }

    fn look(&mut self, out: &FeedTx, runner: Runner<'_>) -> Result<(), Closed> {
        let ps = ask(runner, &["ps", "-a", "--format", "{{json .}}"]);
        let ps = match ps {
            Ok(ran) => ran.stdout,
            Err(why) => return self.status(out, FeedStatus::Error(why.to_string())),
        };
        let stats = ask(runner, &["stats", "--no-stream", "--format", "{{json .}}"]);
        let mut reading = read(&ps, stats.as_ref().ok().map(|ran| ran.stdout.as_slice()));
        if stats.is_err() {
            let note = "docker gave no CPU or memory reading";
            reading.note = Some(
                reading
                    .note
                    .map_or(note.to_owned(), |n| format!("{n}; {note}")),
            );
        }
        self.publish(out, reading)
    }

    /// Carries out one command aimed at a container; a failure becomes the status, kept for the
    /// next reading's note so the look that follows does not clear it.
    fn act(&mut self, out: &FeedTx, runner: Runner<'_>, run: &Run) -> Result<(), Closed> {
        let Some(name) = named(&run.target) else {
            return Ok(());
        };
        if run.command == cmd::LOGS {
            let tail = LOG_LINES.to_string();
            return match ask(runner, &["logs", "--tail", &tail, name]) {
                Ok(ran) => {
                    let both = [ran.stdout, ran.stderr].concat();
                    self.logs.insert(
                        name.to_owned(),
                        log_rows(name, &String::from_utf8_lossy(&both)),
                    );
                    Ok(())
                }
                Err(why) => self.status(out, FeedStatus::Error(why.to_string())),
            };
        }
        let Some(args) = change(run.command, name) else {
            return Ok(());
        };
        match ask(runner, &args) {
            Ok(_) => Ok(()),
            Err(why) => {
                let why = self.refused.insert(why.to_string()).clone();
                self.status(out, FeedStatus::Error(why))
            }
        }
    }
}

enum Wake {
    Look,
    Do(Run),
    Gone,
}

/// Waits for the next thing to do. The timer runs only while `seen`: with no pane showing
/// the feed, nothing is read until something is asked.
fn wait(cmds: &Receiver<SourceCmd>, poll: Duration, seen: &mut bool) -> Wake {
    let end = Instant::now().checked_add(poll);
    loop {
        let left = end.map_or(poll, |end| end.saturating_duration_since(Instant::now()));
        let got = if *seen {
            cmds.recv_timeout(left)
        } else {
            cmds.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        match got {
            Ok(SourceCmd::Run(run)) => return Wake::Do(run),
            Ok(SourceCmd::Seen(on)) => {
                // A pane that has just come into view is read for at once.
                if on && !std::mem::replace(seen, on) {
                    return Wake::Look;
                }
                *seen = on;
            }
            Ok(SourceCmd::Refresh) | Err(RecvTimeoutError::Timeout) => return Wake::Look,
            Ok(_) => {}
            Err(RecvTimeoutError::Disconnected) => return Wake::Gone,
        }
    }
}

/// Reads the containers when a pane comes to show them, every [`POLL`] while one does, on
/// `Refresh`, and after each command it carries out.
pub fn serve(
    out: &FeedTx,
    cmds: &Receiver<SourceCmd>,
    runner: Runner<'_>,
    poll: Duration,
) -> Result<(), Closed> {
    let (mut sent, mut seen) = (Sent::new(), false);
    loop {
        match wait(cmds, poll, &mut seen) {
            Wake::Look => {}
            Wake::Do(run) => sent.act(out, runner, &run)?,
            Wake::Gone => return Ok(()),
        }
        sent.look(out, runner)?;
    }
}

pub fn run(out: FeedTx, cmds: Receiver<SourceCmd>) -> Result<(), Closed> {
    serve(&out, &cmds, &spawn, POLL)
}

#[cfg(test)]
#[rustfmt::skip]
mod tests;
