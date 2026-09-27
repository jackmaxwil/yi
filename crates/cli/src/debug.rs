//! `yi debug`: bug-report verbs (opencode's `debug` family) and the `YI_TRACE` profiler.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{Args, config, default_session_dir, effective_cwd, shells};

const USAGE: &str = "usage: yi debug <verb>
  info              version, platform, terminal, daemon
  paths             every file and directory yi reads or writes
  config            ~/.yi/config.json (secrets redacted) and what it resolves to
  skills            the skills a session in this directory would see
  profile [verb]    run a traced console (or `yi <verb>`) on a private daemon, then report
  trace <dir>       report a YI_TRACE directory and write <dir>/trace.json for Perfetto
  wait              sleep until killed, to attach a profiler";

pub(crate) fn run(args: &Args) -> i32 {
    let mut words = args.prompt.split_whitespace();
    let verb = words.next().unwrap_or_default();
    let rest: Vec<&str> = words.collect();
    match verb {
        "info" => info(args),
        "paths" => paths(args),
        "config" => show_config(args),
        "skills" => skills(args),
        "profile" => profile(args, &rest),
        "trace" => match rest.first() {
            Some(dir) => report(Path::new(dir), args.json),
            None => usage(),
        },
        "wait" => loop {
            std::thread::park();
        },
        _ => usage(),
    }
}

/// The name a traced process carries on the timeline.
pub(crate) fn process_label(command: &str) -> &str {
    match command {
        "serve" => "daemon",
        "acp" => "worker",
        "" => "console",
        verb => verb,
    }
}

fn usage() -> i32 {
    eprintln!("{USAGE}");
    2
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn info(args: &Args) -> i32 {
    let env = |name: &str| std::env::var(name).unwrap_or_default();
    let socket = shells::daemon_socket(args);
    let daemon = if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
        let sessions =
            yi_acp::daemon::read_ledger(&socket).map_or(0, |ledger| ledger.sessions.len());
        format!("listening, {sessions} sessions in the ledger")
    } else {
        "not running".to_owned()
    };
    println!("yi {}", env!("ARCHITECTURE_VERSION"));
    println!("os: {} {}", std::env::consts::OS, std::env::consts::ARCH);
    println!(
        "terminal: {} {} / TERM={} COLORTERM={}",
        env("TERM_PROGRAM"),
        env("TERM_PROGRAM_VERSION"),
        env("TERM"),
        env("COLORTERM")
    );
    println!("model: {}", args.model);
    println!("daemon: {} ({daemon})", socket.display());
    println!(
        "trace: {}",
        std::env::var("YI_TRACE").unwrap_or_else(|_| "off (YI_TRACE=<dir>)".to_owned())
    );
    0
}

fn paths(args: &Args) -> i32 {
    let yi = home().join(".yi");
    let socket = shells::daemon_socket(args);
    let rows = [
        ("home", yi.clone()),
        ("config", yi.join("config.json")),
        ("sessions", default_session_dir(args)),
        ("daemon", socket.clone()),
        ("ledger", socket.with_extension("ledger.json")),
        ("providers", yi.join("providers")),
        ("catalog", yi.join("catalog")),
        ("lanes", yi.join("lanes")),
        ("skills", yi.join("skills")),
        ("memory", yi.join("memory")),
        ("traces", yi.join("traces")),
        ("cwd", effective_cwd(args)),
    ];
    for (name, path) in rows {
        println!("{name:<10} {}", path.display());
    }
    0
}

/// A name that could hold a credential hides its whole value; so does any `sk-` string.
fn redact(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                let key = key.to_ascii_lowercase();
                let secret = ["key", "token", "secret", "password", "auth", "credential"]
                    .iter()
                    .any(|needle| key.contains(needle));
                if secret {
                    *value = json!("<redacted>");
                } else {
                    redact(value);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        Value::String(text) if text.starts_with("sk-") => *value = json!("<redacted>"),
        _ => {}
    }
}

fn show_config(args: &Args) -> i32 {
    let path = home().join(".yi/config.json");
    let mut file = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null);
    redact(&mut file);
    let shown = json!({
        "file": path.display().to_string(),
        "config": file,
        "resolved": {
            "model": args.model,
            "thinking": args.thinking.map(|effort| effort.to_string()),
            "mode": format!("{:?}", args.mode).to_lowercase(),
            "console": {"autoSide": config().console.as_ref().and_then(|console| console.auto_side)},
        },
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&shown).unwrap_or_default()
    );
    0
}

fn skills(args: &Args) -> i32 {
    let found = yi_runtime::skills::discover(&effective_cwd(args), &home());
    if args.json {
        let rows: Vec<Value> = found
            .iter()
            .map(|skill| json!({"name": skill.name, "description": skill.description, "path": skill.path}))
            .collect();
        println!("{}", Value::Array(rows));
        return 0;
    }
    for skill in found {
        println!("{:<24} {}", skill.name, skill.path.display());
        println!("  {}", skill.description);
    }
    0
}

#[expect(
    clippy::disallowed_methods,
    reason = "the profiler re-runs this binary under YI_TRACE and names the trace by the clock"
)]
fn profile(args: &Args, verb: &[&str]) -> i32 {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let dir = home().join(".yi/traces").join(stamp.to_string());
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("error: {}: {error}", dir.display());
        return 1;
    }
    let socket = dir.join("daemon.sock");
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    let status = std::process::Command::new(exe)
        .args(verb)
        .arg("--socket")
        .arg(&socket)
        .args(shells::serve_flags(args))
        .env("YI_TRACE", &dir)
        .status();
    stop_daemon(&socket);
    if let Err(error) = status {
        eprintln!("error: {error}");
        return 1;
    }
    report(&dir, args.json)
}

fn stop_daemon(socket: &Path) {
    use std::io::{BufRead, Write};
    let Ok(mut stream) = std::os::unix::net::UnixStream::connect(socket) else {
        return;
    };
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
    let _ = stream
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"_yi/shutdown\",\"params\":{}}\n");
    let _ = std::io::BufReader::new(stream).read_line(&mut String::new());
}

struct Event {
    name: String,
    phase: String,
    ts: u64,
    dur: u64,
    pid: u64,
    tid: u64,
    args: Value,
}

struct Trace {
    events: Vec<Event>,
    processes: BTreeMap<u64, String>,
    threads: BTreeMap<(u64, u64), String>,
    raw: Vec<Value>,
}

fn load(dir: &Path) -> Result<Trace, String> {
    let entries = std::fs::read_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut trace = Trace {
        events: Vec::new(),
        processes: BTreeMap::new(),
        threads: BTreeMap::new(),
        raw: Vec::new(),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(|error| error.to_string())?;
        for value in text
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        {
            let text_of = |key: &str| {
                value
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            let number = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
            let (pid, tid) = (number("pid"), number("tid"));
            let label = value
                .pointer("/args/name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            match (text_of("ph").as_str(), text_of("name").as_str()) {
                ("M", "process_name") => {
                    trace.processes.insert(pid, label);
                }
                ("M", "thread_name") => {
                    trace.threads.insert((pid, tid), label);
                }
                (phase, name) => trace.events.push(Event {
                    name: name.to_owned(),
                    phase: phase.to_owned(),
                    ts: number("ts"),
                    dur: number("dur"),
                    pid,
                    tid,
                    args: value.get("args").cloned().unwrap_or(Value::Null),
                }),
            }
            trace.raw.push(value);
        }
    }
    if trace.events.is_empty() {
        return Err(format!("{}: no spans (was YI_TRACE set?)", dir.display()));
    }
    trace
        .events
        .sort_by_key(|event| (event.ts, std::cmp::Reverse(event.dur)));
    Ok(trace)
}

fn is_request(event: &Event) -> bool {
    event.name.starts_with("rpc ")
}

fn self_times(events: &[Event]) -> Vec<u64> {
    let mut own: Vec<u64> = events.iter().map(|event| event.dur).collect();
    let mut stacks: BTreeMap<(u64, u64), Vec<usize>> = BTreeMap::new();
    for (index, event) in events.iter().enumerate() {
        if event.phase != "X" || is_request(event) {
            continue;
        }
        let stack = stacks.entry((event.pid, event.tid)).or_default();
        while let Some(&top) = stack.last() {
            let parent = &events[top];
            if parent.ts.saturating_add(parent.dur) > event.ts {
                break;
            }
            stack.pop();
        }
        if let Some(&parent) = stack.last()
            && let Some(slot) = own.get_mut(parent)
        {
            *slot = slot.saturating_sub(event.dur);
        }
        stack.push(index);
    }
    own
}

struct Journey {
    name: String,
    start: u64,
    end: u64,
}

fn console_pid(trace: &Trace) -> Option<u64> {
    trace
        .processes
        .iter()
        .find(|(_, name)| name.starts_with("console"))
        .map(|(pid, _)| *pid)
}

fn journeys(trace: &Trace) -> Vec<Journey> {
    let Some(console) = console_pid(trace) else {
        return Vec::new();
    };
    let on_console = |name: &'static str| {
        trace
            .events
            .iter()
            .filter(move |event| event.pid == console && event.name == name)
    };
    let frame_after = |at: u64| {
        on_console("console.draw")
            .find(|event| event.ts >= at)
            .map(|event| event.ts.saturating_add(event.dur))
    };
    let answered = |name: &'static str, after: u64| {
        on_console(name)
            .find(|event| event.ts >= after)
            .map(|event| event.ts.saturating_add(event.dur))
    };
    let mut found = Vec::new();
    if let Some(main) = on_console("process.main").next()
        && let Some(bound) =
            answered("rpc NewSession", main.ts).or_else(|| answered("rpc Resume", main.ts))
        && let Some(end) = frame_after(bound)
    {
        found.push(Journey {
            name: "startup (console main to first frame with a session)".to_owned(),
            start: main.ts,
            end,
        });
    }
    let switches: Vec<&Event> = on_console("console.switch").collect();
    for (index, switch) in switches.iter().enumerate() {
        let session = switch
            .args
            .get("session")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let next = switches
            .get(index.saturating_add(1))
            .map_or(u64::MAX, |next| next.ts);
        let replayed = on_console("console.replayed").find(|event| {
            event.ts >= switch.ts
                && event.ts < next
                && event.args.get("session").and_then(Value::as_str) == Some(session)
        });
        let entries = replayed
            .and_then(|event| event.args.get("entries"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let ready = replayed
            .map(|event| event.ts)
            .or_else(|| answered("rpc Resume", switch.ts));
        if let Some(end) = ready.and_then(frame_after) {
            let short: String = session.chars().take(8).collect();
            found.push(Journey {
                name: format!("switch to {short} ({entries} entries)"),
                start: switch.ts,
                end,
            });
        }
    }
    found
}

fn ms(micros: u64) -> f64 {
    micros as f64 / 1000.0
}

fn attribute(trace: &Trace, own: &[u64], journey: &Journey) -> Value {
    let mut lanes: BTreeMap<(u64, u64), BTreeMap<String, u64>> = BTreeMap::new();
    let mut requests: Vec<Value> = Vec::new();
    for (event, own) in trace.events.iter().zip(own) {
        let end = event.ts.saturating_add(event.dur);
        if event.phase != "X" || end <= journey.start || event.ts >= journey.end {
            continue;
        }
        if is_request(event) {
            requests.push(json!({"name": event.name, "ms": ms(event.dur)}));
            continue;
        }
        let overlap = end
            .min(journey.end)
            .saturating_sub(event.ts.max(journey.start));
        let share = if event.dur == 0 {
            0
        } else {
            own.saturating_mul(overlap) / event.dur
        };
        *lanes
            .entry((event.pid, event.tid))
            .or_default()
            .entry(event.name.clone())
            .or_default() += share;
    }
    let lanes: Vec<Value> = lanes
        .into_iter()
        .map(|((pid, tid), spans)| {
            let mut spans: Vec<(String, u64)> = spans.into_iter().filter(|(_, micros)| *micros >= 100).collect();
            spans.sort_by_key(|(_, micros)| std::cmp::Reverse(*micros));
            let busy: u64 = spans.iter().map(|(_, micros)| micros).sum();
            json!({
                "process": trace.processes.get(&pid).cloned().unwrap_or_else(|| pid.to_string()),
                "thread": trace.threads.get(&(pid, tid)).cloned().unwrap_or_else(|| format!("t{tid}")),
                "busyMs": ms(busy),
                "spans": spans.into_iter().map(|(name, micros)| json!({"name": name, "ms": ms(micros)})).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "name": journey.name,
        "ms": ms(journey.end.saturating_sub(journey.start)),
        "requests": requests,
        "lanes": lanes,
    })
}

fn report(dir: &Path, as_json: bool) -> i32 {
    let trace = match load(dir) {
        Ok(trace) => trace,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    let perfetto = dir.join("trace.json");
    let merged = json!({"traceEvents": trace.raw, "displayTimeUnit": "ms"});
    let wrote = std::fs::write(&perfetto, merged.to_string()).is_ok();
    let own = self_times(&trace.events);
    let origin = trace.events.iter().map(|event| event.ts).min().unwrap_or(0);
    let processes: Vec<Value> = trace
        .processes
        .iter()
        .map(|(pid, name)| {
            let first = trace
                .events
                .iter()
                .find(|event| event.pid == *pid)
                .map_or(0, |event| event.ts);
            json!({"name": name, "pid": pid, "startMs": ms(first.saturating_sub(origin))})
        })
        .collect();
    let journeys: Vec<Value> = journeys(&trace)
        .iter()
        .map(|journey| attribute(&trace, &own, journey))
        .collect();
    let mut slowest: Vec<(&Event, u64)> = trace
        .events
        .iter()
        .zip(own.iter().copied())
        .filter(|(event, _)| event.phase == "X" && !is_request(event))
        .collect();
    slowest.sort_by_key(|(_, own)| std::cmp::Reverse(*own));
    let slowest: Vec<Value> = slowest
        .iter()
        .take(12)
        .map(|(event, own)| {
            json!({
                "name": event.name,
                "process": trace.processes.get(&event.pid).cloned().unwrap_or_default(),
                "atMs": ms(event.ts.saturating_sub(origin)),
                "selfMs": ms(*own),
                "args": event.args,
            })
        })
        .collect();
    let summary = json!({
        "dir": dir.display().to_string(),
        "perfetto": wrote.then(|| perfetto.display().to_string()),
        "processes": processes,
        "journeys": journeys,
        "slowest": slowest,
    });
    if as_json {
        println!("{summary}");
    } else {
        print_report(&summary);
    }
    0
}

fn print_report(summary: &Value) {
    let list = |key: &str| {
        summary
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let text = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let number = |value: &Value, key: &str| value.get(key).and_then(Value::as_f64).unwrap_or(0.0);
    println!("trace {}", text(summary, "dir"));
    for process in list("processes") {
        println!(
            "  {:<28} starts +{:.1} ms",
            text(&process, "name"),
            number(&process, "startMs")
        );
    }
    for journey in list("journeys") {
        println!(
            "\n{}: {:.1} ms",
            text(&journey, "name"),
            number(&journey, "ms")
        );
        let requests: Vec<String> = journey
            .get("requests")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|request| format!("{} {:.1}", text(request, "name"), number(request, "ms")))
            .collect();
        if !requests.is_empty() {
            println!("  {:<24} {}", "requests", requests.join(" · "));
        }
        for lane in journey
            .get("lanes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let spans: Vec<String> = lane
                .get("spans")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .take(6)
                .map(|span| format!("{} {:.1}", text(span, "name"), number(span, "ms")))
                .collect();
            if spans.is_empty() {
                continue;
            }
            let who = format!("{} {}", text(lane, "process"), text(lane, "thread"));
            println!("  {who:<24} {}", spans.join(" · "));
        }
    }
    println!("\nslowest spans (self time)");
    for span in list("slowest") {
        let args = span
            .get("args")
            .filter(|args| args.as_object().is_some_and(|map| !map.is_empty()));
        println!(
            "  {:>8.1} ms  +{:>9.1}  {:<22} {} {}",
            number(&span, "selfMs"),
            number(&span, "atMs"),
            text(&span, "process"),
            text(&span, "name"),
            args.map(Value::to_string).unwrap_or_default()
        );
    }
    if let Some(path) = summary.get("perfetto").and_then(Value::as_str) {
        println!("\nperfetto: {path} (open in https://ui.perfetto.dev)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(name: &str, ts: u64, dur: u64) -> Event {
        Event {
            name: name.to_owned(),
            phase: "X".to_owned(),
            ts,
            dur,
            pid: 1,
            tid: 1,
            args: Value::Null,
        }
    }

    #[test]
    fn config_secrets_never_print() {
        let mut config = json!({
            "model": "openrouter/x",
            "keys": {"openrouter": "sk-or-v1-abc"},
            "mcp": {"servers": [{"env": {"GITHUB_TOKEN": "ghp_x"}, "note": "sk-live"}]},
        });
        redact(&mut config);
        let shown = config.to_string();
        for secret in ["sk-or-v1-abc", "ghp_x", "sk-live"] {
            assert!(!shown.contains(secret), "{shown}");
        }
        assert!(shown.contains("openrouter/x"), "{shown}");
    }

    #[test]
    fn journeys_end_at_the_frame_after_the_session_shows() {
        let marker = |name: &str, ts: u64, dur: u64, session: Option<&str>| {
            let mut event = span(name, ts, dur);
            if let Some(session) = session {
                event.args = json!({"session": session});
            }
            event
        };
        let events = vec![
            marker("process.main", 0, 0, None),
            marker("console.draw", 5, 3, None),
            marker("rpc NewSession", 10, 40, None),
            marker("console.draw", 60, 4, None),
            marker("console.switch", 100, 0, Some("abc")),
            marker("console.draw", 101, 2, None),
            marker("console.replayed", 150, 0, Some("abc")),
            marker("console.draw", 160, 6, None),
        ];
        let trace = Trace {
            events,
            processes: BTreeMap::from([(1, "console [1]".to_owned())]),
            threads: BTreeMap::new(),
            raw: Vec::new(),
        };
        let found: Vec<(String, u64)> = journeys(&trace)
            .into_iter()
            .map(|journey| (journey.name, journey.end - journey.start))
            .collect();
        assert_eq!(
            found,
            vec![
                (
                    "startup (console main to first frame with a session)".to_owned(),
                    64
                ),
                ("switch to abc (0 entries)".to_owned(), 66),
            ]
        );
    }

    #[test]
    fn nested_spans_leave_the_parent_only_its_own_time() {
        let events = vec![
            span("outer", 0, 100),
            span("inner", 10, 30),
            span("rpc Resume", 20, 500),
            span("sibling", 50, 20),
            span("after", 200, 5),
        ];
        assert_eq!(self_times(&events), vec![50, 30, 500, 20, 5]);
    }
}
