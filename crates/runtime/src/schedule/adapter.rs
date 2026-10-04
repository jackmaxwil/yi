use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::channel::{Appended, Channel};

const PREFIX: &str = "yi-adapter-";
const NAP: Duration = Duration::from_millis(250);
const EXEC_TIMEOUT_MS: u64 = 30_000;
const LINE_MAX_BYTES: u64 = 64 * 1024;
const LOG_MAX_BYTES: u64 = 1024 * 1024;

/// One adapter per channel per process, alive while a subscription ticks within its lease;
/// `dead` holds why it stopped past its restart intensity until a resume revives it.
struct Supervisor {
    deadline: Mutex<Instant>,
    /// Until then a subscription that creates todos reads the channel, so a built-in source
    /// keeps its own cadence; past it only waits read, and an unchanged level backs off.
    eager_until: Mutex<Instant>,
    stop: AtomicBool,
    dead: Mutex<Option<String>>,
}

#[derive(Default)]
struct Registry {
    halted: bool,
    home: Option<PathBuf>,
    running: HashMap<PathBuf, Arc<Supervisor>>,
}

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn held<T>(lock: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    lock.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The home whose `.yi/adapters/` is searched before `PATH`; unset, `$HOME` is.
pub fn adapters_home(home: &Path) {
    registry().home = Some(home.join(".yi").join("adapters"));
}

pub fn scheme(uri: &str) -> Option<&str> {
    let (scheme, _) = uri.split_once("://")?;
    let valid = !scheme.is_empty()
        && scheme
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '+' | '-'));
    valid.then_some(scheme)
}

/// A source's own cadence, `?every=<n>s|m|h` at the end of its URI, one second at least.
pub fn every(uri: &str) -> Option<u64> {
    let (_, text) = uri.rsplit_once("?every=")?;
    let (amount, unit) = super::split_amount_unit(text)?;
    let unit_ms = match unit {
        "s" => super::ONE_SECOND_MS,
        "m" => super::ONE_MINUTE_MS,
        "h" => super::ONE_MINUTE_MS.saturating_mul(60),
        _ => return None,
    };
    amount
        .checked_mul(unit_ms)
        .filter(|ms| *ms >= super::ONE_SECOND_MS)
}

fn command_of(uri: &str, scheme: &str) -> String {
    let rest = uri
        .strip_prefix(scheme)
        .and_then(|rest| rest.strip_prefix("://"))
        .unwrap_or_default();
    match rest.rsplit_once("?every=") {
        Some((head, _)) if every(uri).is_some() => head,
        _ => rest,
    }
    .to_owned()
}

pub fn exec_command(address: &str) -> Option<String> {
    (scheme(address) == Some("exec")).then(|| command_of(address, "exec"))
}

pub fn find(scheme: &str) -> Option<PathBuf> {
    let name = format!("{PREFIX}{scheme}");
    let home = registry().home.clone().or_else(|| {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".yi").join("adapters"))
    });
    let path: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    home.into_iter()
        .chain(path)
        .map(|dir| dir.join(&name))
        .find(|candidate| candidate.is_file())
}

/// A source the host can run: `exec` and `file` are built in, anything else is an adapter found
/// by its scheme, refused by name when none is installed.
pub fn check(uri: &str) -> Result<(), String> {
    let scheme =
        scheme(uri).ok_or_else(|| format!("{uri:?} is not an adapter URI `<scheme>://…`"))?;
    match scheme {
        "exec" | "file" if command_of(uri, scheme).trim().is_empty() => {
            Err(format!("{uri:?} names no {scheme} target"))
        }
        "exec" | "file" | "clock" => Ok(()),
        _ => find(scheme).map(drop).ok_or_else(|| {
            format!("no adapter for {scheme}://: put an executable {PREFIX}{scheme} in ~/.yi/adapters/ or on PATH")
        }),
    }
}

/// Starts the channel's adapter unless one runs, or extends its lease; an adapter past its
/// restart intensity answers why. Nothing starts while the kill switch holds.
pub fn ensure(
    uri: &str,
    channel: &Channel,
    cwd: &Path,
    cadence_ms: u64,
    eager: bool,
) -> Result<(), String> {
    let lease = Duration::from_millis(cadence_ms.saturating_mul(3)).max(Duration::from_secs(30));
    let now = Instant::now();
    let deadline = now.checked_add(lease).unwrap_or(now);
    let eager_until = if eager { deadline } else { now };
    let mut registry = registry();
    if registry.halted
        || uri.starts_with(super::clock::CLOCK_SCHEME)
        || uri.starts_with(super::channel::CHANNEL_SCHEME)
    {
        return Ok(());
    }
    if let Some(running) = registry.running.get(channel.path()) {
        if let Some(why) = held(&running.dead).clone() {
            return Err(why);
        }
        *held(&running.deadline) = deadline;
        if eager {
            *held(&running.eager_until) = eager_until;
        }
        return Ok(());
    }
    let supervisor = Arc::new(Supervisor {
        deadline: Mutex::new(deadline),
        eager_until: Mutex::new(eager_until),
        stop: AtomicBool::new(false),
        dead: Mutex::new(None),
    });
    registry
        .running
        .insert(channel.path().to_path_buf(), Arc::clone(&supervisor));
    drop(registry);
    let (uri, channel, cwd) = (uri.to_owned(), channel.clone(), cwd.to_path_buf());
    let spawned = std::thread::Builder::new()
        .name("yi-adapter".to_owned())
        .spawn(move || supervise(&uri, &channel, &cwd, &supervisor));
    spawned
        .map(drop)
        .map_err(|error| format!("adapter thread did not start: {error}"))
}

/// A resumed subscription gives a dead adapter a fresh intensity.
pub fn revive(channel: &Path) {
    let mut registry = registry();
    if registry
        .running
        .get(channel)
        .is_some_and(|running| held(&running.dead).is_some())
    {
        registry.running.remove(channel);
    }
}

/// The kill switch: `true` stops every adapter and starts none until `false`; answers how many
/// it stopped.
pub fn halt(on: bool) -> u64 {
    let mut registry = registry();
    registry.halted = on;
    if !on {
        return 0;
    }
    let stopped = registry
        .running
        .drain()
        .filter(|(_, running)| !running.stop.swap(true, Ordering::SeqCst))
        .count();
    u64::try_from(stopped).unwrap_or(u64::MAX)
}

enum Ended {
    Lapsed,
    Crashed(String),
    Missing(String),
}

fn lapsed(supervisor: &Supervisor) -> bool {
    supervisor.stop.load(Ordering::SeqCst) || Instant::now() >= *held(&supervisor.deadline)
}

fn supervise(uri: &str, channel: &Channel, cwd: &Path, supervisor: &Arc<Supervisor>) {
    let max = usize::try_from(crate::subagent::service::DEFAULT_RESTARTS).unwrap_or(usize::MAX);
    let mut restarts = Vec::new();
    let dead = loop {
        let ended = match scheme(uri) {
            Some("exec") => {
                let command = command_of(uri, "exec");
                poll(
                    uri,
                    channel,
                    supervisor,
                    || exec(&command, cwd),
                    |data| data.get("ok").cloned().unwrap_or_default(),
                )
            }
            Some("file") => {
                let path = cwd.join(command_of(uri, "file"));
                poll(uri, channel, supervisor, || file(&path), Value::clone)
            }
            _ => external(uri, channel, cwd, supervisor),
        };
        match ended {
            Ended::Lapsed => break None,
            Ended::Missing(why) => break Some(format!("adapter for {uri} stopped: {why}")),
            Ended::Crashed(why) => {
                if let Err(spent) = crate::subagent::service::spend_restart(
                    &mut restarts,
                    max,
                    yi_session::now_ms(),
                ) {
                    break Some(format!(
                        "adapter for {uri} stopped: {why}, and its restart intensity is spent ({spent})"
                    ));
                }
                std::thread::sleep(NAP);
            }
        }
    };
    let mut registry = registry();
    match dead {
        Some(why) => *held(&supervisor.dead) = Some(why),
        None => registry
            .running
            .retain(|_, running| !Arc::ptr_eq(running, supervisor)),
    }
}

fn nap(supervisor: &Supervisor, span: Duration) -> bool {
    let until = Instant::now()
        .checked_add(span)
        .unwrap_or_else(Instant::now);
    while Instant::now() < until {
        if lapsed(supervisor) {
            return true;
        }
        std::thread::sleep(NAP.min(until.saturating_duration_since(Instant::now())));
    }
    lapsed(supervisor)
}

/// Invariant: a source emits only a changed level (its last is the channel's newest entry), and
/// read only by waits it doubles its cadence up to `plan.probe_max_s` until the level changes.
fn poll(
    uri: &str,
    channel: &Channel,
    supervisor: &Supervisor,
    mut level: impl FnMut() -> Value,
    key: impl Fn(&Value) -> Value,
) -> Ended {
    let base = Duration::from_millis(every(uri).unwrap_or(super::channel::DEFAULT_CADENCE_MS));
    let cap = base.max(Duration::from_secs(crate::levers::get().plan_probe_max_s));
    let mut span = base;
    let mut last = channel.last().map(|entry| key(&entry.data));
    loop {
        if lapsed(supervisor) {
            return Ended::Lapsed;
        }
        let data = level();
        let now = key(&data);
        let eager = Instant::now() < *held(&supervisor.eager_until);
        span = if eager {
            base
        } else {
            span.saturating_mul(2).min(cap)
        };
        if last.as_ref() != Some(&now) {
            let at = yi_session::now_ms();
            if let Ok(appended) = channel.append(&at.to_string(), at, data) {
                landed(channel, &appended);
                last = Some(now);
                span = base;
            }
        }
        if nap(supervisor, span) {
            return Ended::Lapsed;
        }
    }
}

/// Emits on a change of exit status, not per output line: a check is a level, red or green,
/// and a line stream would re-send the same log on every run of an unchanged check.
fn exec(command: &str, cwd: &Path) -> Value {
    let mut shell = yi_tools::keyless_command("sh");
    shell.arg("-c").arg(command).current_dir(cwd);
    let deadline = Instant::now()
        .checked_add(Duration::from_millis(EXEC_TIMEOUT_MS))
        .unwrap_or_else(Instant::now);
    let cancelled: yi_tools::CancelFlag = Arc::new(move || Instant::now() >= deadline);
    match yi_tools::run_captured(shell, None, &cancelled, 30_000) {
        Ok(capture) if capture.exit_code == Some(0) => json!({"ok": true, "exit": 0}),
        Ok(capture) => json!({
            "ok": false,
            "exit": capture.exit_code,
            "output": crate::goal::output_tail(&capture),
        }),
        Err(error) => json!({"ok": false, "output": format!("did not run: {error}")}),
    }
}

fn file(path: &Path) -> Value {
    let modified = |meta: &std::fs::Metadata| {
        meta.modified()
            .ok()
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
    };
    match std::fs::metadata(path) {
        Ok(meta) => json!({
            "path": path.to_string_lossy(),
            "exists": true,
            "size": meta.len(),
            "mtimeMs": modified(&meta),
        }),
        Err(_) => json!({"path": path.to_string_lossy(), "exists": false}),
    }
}

/// The protocol: the adapter writes `{id, at, data}` lines on stdout; the host appends each and
/// only then writes `{"ack": id}` on stdin, adding `refused` when the home kept a refusal.
fn external(uri: &str, channel: &Channel, cwd: &Path, supervisor: &Supervisor) -> Ended {
    let Some(scheme) = scheme(uri) else {
        return Ended::Missing(format!("{uri:?} has no scheme"));
    };
    let Some(program) = find(scheme) else {
        return Ended::Missing(format!("no {PREFIX}{scheme} in ~/.yi/adapters/ or on PATH"));
    };
    let log_path = channel.path().with_extension("log");
    // ponytail: capped per start; an adapter that spews without exiting grows it until then.
    let full = std::fs::metadata(&log_path).is_ok_and(|meta| meta.len() > LOG_MAX_BYTES);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(!full)
        .truncate(full)
        .open(&log_path);
    let spawned = yi_tools::command(&program)
        .arg(uri)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(log.map_or_else(|_| Stdio::null(), Stdio::from))
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            return Ended::Crashed(format!("{} did not start: {error}", program.display()));
        }
    };
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return reap(child, "had no stdio");
    };
    let (lines, read) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        while let Some(line) = read_line(&mut reader) {
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    loop {
        if lapsed(supervisor) {
            let _already_exited = child.kill();
            let _reaped = child.wait();
            return Ended::Lapsed;
        }
        match read.recv_timeout(NAP) {
            Ok(line) => {
                if let Some(reply) = take(channel, &line) {
                    let _gone_adapter_exits_and_is_reaped = writeln!(stdin, "{reply}");
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) if lapsed(supervisor) => {}
            Err(RecvTimeoutError::Disconnected) => return reap(child, "closed its stdout"),
        }
    }
}

/// Invariant: a line holds at most [`LINE_MAX_BYTES`]; the rest of a longer one is read and
/// dropped, so the cut line fails to parse and is kept as `{malformed}` rather than filling memory.
fn read_line(reader: &mut impl BufRead) -> Option<String> {
    let mut line = Vec::new();
    match reader.take(LINE_MAX_BYTES).read_until(b'\n', &mut line) {
        Ok(0) | Err(_) => return None,
        Ok(_) => {}
    }
    let mut rest = Vec::new();
    while !line.ends_with(b"\n") && !rest.ends_with(b"\n") {
        rest.clear();
        match reader.take(LINE_MAX_BYTES).read_until(b'\n', &mut rest) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
    Some(String::from_utf8_lossy(&line).trim_end().to_owned())
}

fn reap(mut child: Child, why: &str) -> Ended {
    let _already_exited = child.kill();
    let status = child
        .wait()
        .map_or_else(|error| error.to_string(), |status| status.to_string());
    Ended::Crashed(format!("the adapter {why} ({status})"))
}

/// A line that is not `{id, at, data}` is kept as `{malformed}` data, so a broken adapter shows.
fn take(channel: &Channel, line: &str) -> Option<Value> {
    let parsed: Option<Value> = serde_json::from_str(line).ok();
    let id = parsed
        .as_ref()
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str);
    let now = yi_session::now_ms();
    let Some(id) = id else {
        let head: String = line.chars().take(200).collect();
        let refusal = json!({"malformed": head});
        let _logged_as_entry = channel.append(&format!("malformed-{now}"), now, refusal);
        return None;
    };
    let at = parsed
        .as_ref()
        .and_then(|value| value.get("at"))
        .and_then(Value::as_u64)
        .unwrap_or(now);
    let data = parsed
        .as_ref()
        .and_then(|value| value.get("data"))
        .cloned()
        .unwrap_or_default();
    let appended = channel.append(id, at, data).ok()?;
    landed(channel, &appended);
    Some(match appended {
        Appended::Kept(_) | Appended::Duplicate => json!({"ack": id}),
        Appended::Refused(why) => json!({"ack": id, "refused": why}),
    })
}

fn landed(channel: &Channel, appended: &Appended) {
    if !matches!(appended, Appended::Duplicate) {
        super::shared::poke(channel.path());
    }
}
