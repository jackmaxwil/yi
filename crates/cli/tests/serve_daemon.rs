use std::error::Error;
use std::io::{BufRead, BufReader, Lines, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yi_types::schedule::ScheduleState;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

const WORKERS: usize = 2;
const SESSIONS_PER_WORKER: usize = 2;

struct DaemonClient {
    stream: UnixStream,
    lines: Lines<BufReader<UnixStream>>,
}

impl DaemonClient {
    fn connect(socket: &Path) -> Result<Self, Box<dyn Error>> {
        let stream = UnixStream::connect(socket)?;
        let reader = stream.try_clone()?;
        // A frame that never arrives must fail the test, not hang the suite
        // until CI's own timeout kills it with no evidence.
        reader.set_read_timeout(Some(Duration::from_secs(30)))?;
        Ok(Self {
            stream,
            lines: BufReader::new(reader).lines(),
        })
    }

    fn send(&mut self, id: &str, method: &str, params: Value) -> Result<(), Box<dyn Error>> {
        let frame = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        serde_json::to_writer(&mut self.stream, &frame)?;
        self.stream.write_all(b"\n")?;
        Ok(())
    }

    fn read_until(&mut self, stop: impl Fn(&Value) -> bool) -> Result<Vec<Value>, Box<dyn Error>> {
        let mut seen = Vec::new();
        for line in self.lines.by_ref() {
            let frame: Value = serde_json::from_str(&line?)?;
            let done = stop(&frame);
            seen.push(frame);
            if done {
                return Ok(seen);
            }
        }
        Err(format!("stream ended before the expected frame; saw {seen:?}").into())
    }

    fn request(&mut self, id: &str, method: &str, params: Value) -> Result<Value, Box<dyn Error>> {
        let frame = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        serde_json::to_writer(&mut self.stream, &frame)?;
        self.stream.write_all(b"\n")?;
        for line in self.lines.by_ref() {
            let frame: Value = serde_json::from_str(&line?)?;
            if frame["id"] == id {
                return Ok(frame);
            }
        }
        Err("stream ended before the response".into())
    }

    /// Every frame up to and including a response, so a notification the worker sent before
    /// answering is part of the evidence rather than something the reader skipped.
    fn request_with_notifications(
        &mut self,
        id: &str,
        method: &str,
        params: Value,
    ) -> Result<Vec<Value>, Box<dyn Error>> {
        let frame = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        serde_json::to_writer(&mut self.stream, &frame)?;
        self.stream.write_all(b"\n")?;
        self.read_until(|frame| frame["id"] == id)
    }
}

fn spawn_daemon(dir: &Path) -> Result<(Child, PathBuf), Box<dyn Error>> {
    spawn_daemon_in(dir, None)
}

fn spawn_daemon_in(dir: &Path, home: Option<&Path>) -> Result<(Child, PathBuf), Box<dyn Error>> {
    let socket = dir.join("yi.sock");
    #[expect(
        clippy::disallowed_methods,
        reason = "the daemon contract is the spawned binary's socket; tests must drive the real process"
    )]
    let child = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "serve",
            "--socket",
            &socket.display().to_string(),
            "--model",
            "faux/faux-1",
            "--session-dir",
            &dir.join("sessions").display().to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .envs(home.map(|home| ("HOME", home.to_path_buf())))
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    // Incident: the socket file exists between bind() and listen(), and a
    // connect in that gap is refused; under load the gap outlasted the poll.
    while UnixStream::connect(&socket).is_err() {
        if Instant::now() > deadline {
            return Err("daemon socket never accepted a connection".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok((child, socket))
}

/// A root session's jobs live in `<sessions>/schedules/<session id>/`, so each
/// armed session has its own ledger that outlives the worker's pid.
fn ledger_dirs(sessions: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let schedules = sessions.join("schedules");
    let mut dirs = Vec::new();
    if !schedules.is_dir() {
        return Ok(dirs);
    }
    for entry in std::fs::read_dir(schedules)? {
        dirs.push(entry?.path());
    }
    Ok(dirs)
}

fn ledger_path(dir: &Path) -> PathBuf {
    dir.join("scheduled-jobs.json")
}

/// `runCount` is always serialized, so a text match passes on a job that never fired.
fn dispatched(dir: &Path) -> bool {
    std::fs::read_to_string(ledger_path(dir))
        .ok()
        .and_then(|text| serde_json::from_str::<ScheduleState>(&text).ok())
        .is_some_and(|state| {
            !state.dispatches.is_empty() || state.jobs.iter().any(|job| job.run_count >= 1)
        })
}

fn any_heartbeat_dispatched(sessions: &Path) -> Result<bool, Box<dyn Error>> {
    Ok(ledger_dirs(sessions)?.iter().any(|dir| dispatched(dir)))
}

fn wait_until(
    deadline: Instant,
    mut check: impl FnMut() -> Result<bool, Box<dyn Error>>,
) -> TestResult {
    loop {
        if check()? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("deadline elapsed before the daemon reached the expected state".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn session_id_from(new: &Value) -> Result<String, Box<dyn Error>> {
    new["result"]["sessionId"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "missing sessionId".into())
}

fn arm_heartbeat(client: &mut DaemonClient, id: &str, session_id: &str) -> TestResult {
    let heartbeat = client.request(
        id,
        "_yi/heartbeat",
        json!({
            "sessionId": session_id,
            "command": "--every 10s check on the build",
        }),
    )?;
    if heartbeat["result"]["text"]
        .as_str()
        .is_some_and(|text| !text.is_empty())
    {
        Ok(())
    } else {
        Err(format!("the heartbeat surface must acknowledge: {heartbeat}").into())
    }
}

#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn reconnect_keeps_heartbeats() -> TestResult {
    let dir = Scratch::new("yi-serve")?;
    let (mut daemon, socket) = spawn_daemon(&dir)?;

    let outcome = (|| -> TestResult {
        let mut client = DaemonClient::connect(&socket)?;
        let init = client.request("1", "initialize", json!({"protocolVersion": 2}))?;
        assert_eq!(init["result"]["protocolVersion"], 2);

        let new = client.request(
            "2",
            "session/new",
            json!({"cwd": dir.display().to_string()}),
        )?;
        let session_id = session_id_from(&new)?;
        arm_heartbeat(&mut client, "3", &session_id)?;
        drop(client);

        wait_until(Instant::now() + Duration::from_secs(20), || {
            any_heartbeat_dispatched(&dir.join("sessions"))
        })?;

        let mut reconnected = DaemonClient::connect(&socket)?;
        reconnected.request("1", "initialize", json!({"protocolVersion": 2}))?;
        let list = reconnected.request("2", "session/list", json!({}))?;
        assert!(
            list["result"]["sessions"]
                .as_array()
                .is_some_and(|sessions| {
                    sessions
                        .iter()
                        .any(|entry| entry["sessionId"] == session_id.as_str())
                }),
            "the supervisor must still know the session after reconnect: {list}"
        );
        let resume = reconnected.request(
            "3",
            "session/resume",
            json!({"sessionId": session_id, "cwd": dir.display().to_string()}),
        )?;
        assert!(
            resume["result"]["configOptions"].is_array(),
            "resume must route to the surviving worker: {resume}"
        );
        Ok(())
    })();

    let _cleanup = daemon.kill();
    let _reaped = daemon.wait();
    outcome
}

fn new_session(
    client: &mut DaemonClient,
    id: &str,
    root: &std::path::Path,
) -> Result<String, Box<dyn Error>> {
    let new = client.request(
        id,
        "session/new",
        json!({"cwd": root.display().to_string()}),
    )?;
    session_id_from(&new)
}

/// Two roots must each end up with a scheduler that keeps firing detached: one
/// worker serving both collapses the two dispatch records into one, which is
/// what a single-worker regression looks like from outside.
#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn two_roots_run_two_workers_that_keep_their_own_schedules() -> TestResult {
    let dir = Scratch::new("yi-serve-roots")?;
    let alpha = dir.join("alpha");
    let beta = dir.join("beta");
    std::fs::create_dir_all(&alpha)?;
    std::fs::create_dir_all(&beta)?;
    let (mut daemon, socket) = spawn_daemon(&dir)?;

    let outcome = (|| -> TestResult {
        let mut client = DaemonClient::connect(&socket)?;
        client.request("1", "initialize", json!({"protocolVersion": 2}))?;
        let first = new_session(&mut client, "2", &alpha)?;
        let second = new_session(&mut client, "3", &beta)?;
        assert_ne!(first, second, "each root gets its own session");

        for (id, session) in [("4", &first), ("5", &second)] {
            client.send(
                id,
                "session/prompt",
                json!({"sessionId": session, "prompt": [{"type": "text", "text": "sanity"}]}),
            )?;
            let frames = client.read_until(|frame| {
                frame["params"]["sessionId"] == session.as_str()
                    && frame["params"]["update"]["sessionUpdate"] == "state_update"
                    && frame["params"]["update"]["state"] == "idle"
            })?;
            let text: String = frames
                .iter()
                .filter(|frame| frame["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
                .filter_map(|frame| frame["params"]["update"]["content"]["text"].as_str())
                .collect();
            assert!(
                text.contains("faux:"),
                "the turn in root {session} must stream back through the supervisor: {text:?}"
            );
            client.request(
                id,
                "_yi/heartbeat",
                json!({"sessionId": session, "command": "--every 10s check on the build"}),
            )?;
        }
        drop(client);

        // Both schedulers must keep firing with nobody attached, which is the
        // whole point of a worker that outlives its client.
        std::thread::sleep(Duration::from_secs(13));
        let fired = ledger_dirs(&dir.join("sessions"))?
            .iter()
            .filter(|ledger| dispatched(ledger))
            .count();
        assert_eq!(
            fired, 2,
            "both roots must have dispatched while no client was attached"
        );

        let mut reconnected = DaemonClient::connect(&socket)?;
        reconnected.request("1", "initialize", json!({"protocolVersion": 2}))?;
        let list = reconnected.request("2", "session/list", json!({}))?;
        let listed = list["result"]["sessions"]
            .as_array()
            .ok_or("sessions must be an array")?;
        for session in [&first, &second] {
            let entry = listed
                .iter()
                .find(|entry| entry["sessionId"] == session.as_str())
                .ok_or_else(|| format!("the supervisor must still know {session}: {list}"))?;
            assert_eq!(
                entry["title"], "sanity",
                "the ledger names a session from its first prompt: {entry}"
            );
            let resume =
                reconnected.request("3", "session/resume", json!({"sessionId": session}))?;
            assert!(
                resume["result"]["configOptions"].is_array(),
                "resume must route to the surviving worker: {resume}"
            );
        }
        Ok(())
    })();

    let _cleanup = daemon.kill();
    let _reaped = daemon.wait();
    outcome
}

/// Two roots spawn two workers. Each ACP session keeps its heartbeat in its
/// own `schedules/<session id>/` ledger, which its worker's timer drives.
/// The failure is a sibling worker whose jobs never run, a torn
/// `scheduled-jobs.json`, or the second session cancelling the first.
#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn workers_do_not_lose_heartbeats_or_tear_the_job_ledger() -> TestResult {
    let dir = Scratch::new("yi-serve-g2")?;
    let (mut daemon, socket) = spawn_daemon(&dir)?;

    let outcome = (|| -> TestResult {
        let mut client = DaemonClient::connect(&socket)?;
        client.request("init", "initialize", json!({"protocolVersion": 2}))?;

        let mut armed = Vec::new();
        let mut req = 0u32;
        for worker in 0..WORKERS {
            let cwd = dir.join(format!("root-{worker}"));
            std::fs::create_dir_all(&cwd)?;
            for _ in 0..SESSIONS_PER_WORKER {
                req = req.saturating_add(1);
                let new = client.request(
                    &format!("new-{req}"),
                    "session/new",
                    json!({"cwd": cwd.display().to_string()}),
                )?;
                let session_id = session_id_from(&new)?;
                req = req.saturating_add(1);
                arm_heartbeat(&mut client, &format!("hb-{req}"), &session_id)?;
                armed.push((session_id, cwd.display().to_string()));
            }
        }
        drop(client);

        let sessions = dir.join("sessions");
        if let Err(error) = wait_until(Instant::now() + Duration::from_secs(25), || {
            ledgers_are_intact(&sessions)
        }) {
            return Err(format!("{error}\n{}", ledger_dump(&sessions)?).into());
        }

        let mut reconnected = DaemonClient::connect(&socket)?;
        reconnected.request("1", "initialize", json!({"protocolVersion": 2}))?;
        let list = reconnected.request("2", "session/list", json!({}))?;
        let listed = list["result"]["sessions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for (session_id, cwd) in &armed {
            assert!(
                listed
                    .iter()
                    .any(|entry| entry["sessionId"] == session_id.as_str()),
                "every armed session must survive reconnect: missing {session_id} in {list}"
            );
            let resume = reconnected.request(
                session_id,
                "session/resume",
                json!({"sessionId": session_id, "cwd": cwd}),
            )?;
            assert!(
                resume["result"]["configOptions"].is_array(),
                "resume must route to the surviving worker: {resume}"
            );
        }
        Ok(())
    })();

    let _cleanup = daemon.kill();
    let _reaped = daemon.wait();
    outcome
}

fn ledger_dump(sessions: &Path) -> Result<String, Box<dyn Error>> {
    let dirs = ledger_dirs(sessions)?;
    let mut out = format!("ledgers: {}\n", dirs.len());
    for dir in dirs {
        let path = ledger_path(&dir);
        let body = std::fs::read_to_string(&path).unwrap_or_else(|error| format!("read: {error}"));
        out.push_str(&format!("{}:\n{body}\n", path.display()));
    }
    Ok(out)
}

fn ledgers_are_intact(sessions: &Path) -> Result<bool, Box<dyn Error>> {
    let dirs = ledger_dirs(sessions)?;
    if dirs.len() != WORKERS * SESSIONS_PER_WORKER {
        return Ok(false);
    }
    for dir in dirs {
        let text = match std::fs::read_to_string(ledger_path(&dir)) {
            Ok(text) => text,
            Err(_) => return Ok(false),
        };
        let state: ScheduleState = match serde_json::from_str(&text) {
            Ok(state) => state,
            Err(_) => return Ok(false),
        };
        if !state.dispatches.is_empty() {
            return Ok(false);
        }
        let owner = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
        let ran: Vec<_> = state.jobs.iter().filter(|job| job.run_count >= 1).collect();
        if ran.len() != 1
            || ran
                .iter()
                .any(|job| Some(&job.session_id) != owner.as_ref())
        {
            return Ok(false);
        }
    }
    Ok(true)
}

/// One session, two attached clients: both must receive the same update
/// stream. A single attached-pointer regression sends the fan-out to only
/// the last resumer.
#[test]
fn two_clients_both_stream_one_session() -> TestResult {
    let dir = Scratch::new("yi-fanout")?;
    let root = dir.join("repo");
    std::fs::create_dir_all(&root)?;
    let (mut daemon, socket) = spawn_daemon(&dir)?;

    let outcome = (|| -> TestResult {
        let mut first = DaemonClient::connect(&socket)?;
        first.request("i1", "initialize", json!({"protocolVersion": 2}))?;
        let session_id = new_session(&mut first, "n1", &root)?;

        let mut second = DaemonClient::connect(&socket)?;
        second.request("i2", "initialize", json!({"protocolVersion": 2}))?;
        second.request(
            "r2",
            "session/resume",
            json!({"sessionId": session_id, "cwd": root.display().to_string(), "replayFrom": 0}),
        )?;

        first.send(
            "p1",
            "session/prompt",
            json!({"sessionId": session_id,
                "prompt": [{"type": "text", "text": "fan this out"}]}),
        )?;

        let is_chunk = |frame: &Value| {
            frame["method"] == "session/update"
                && frame["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
        };
        first.read_until(is_chunk)?;
        second.read_until(is_chunk)?;
        Ok(())
    })();

    let _ = daemon.kill();
    let _ = daemon.wait();
    outcome
}

/// A client that leaves mid-turn is owed one unseen mark for the idle it missed, not one
/// per streamed event, and the client that comes back gets the branch verbatim.
#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn a_returning_client_sees_one_unseen_and_the_branch_verbatim() -> TestResult {
    let dir = Scratch::new("yi-serve-replay")?;
    let (mut daemon, socket) = spawn_daemon(&dir)?;

    let outcome = (|| -> TestResult {
        let mut first = DaemonClient::connect(&socket)?;
        first.request("1", "initialize", json!({"protocolVersion": 2}))?;
        let session_id = new_session(&mut first, "2", &dir)?;
        first.send(
            "3",
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "fan this out"}]}),
        )?;
        drop(first);

        let mut second = DaemonClient::connect(&socket)?;
        second.request("1", "initialize", json!({"protocolVersion": 2}))?;
        let entry = wait_until(Instant::now() + Duration::from_secs(20), || {
            let list = second.request("2", "session/list", json!({}))?;
            Ok(list["result"]["sessions"]
                .as_array()
                .and_then(|rows| {
                    rows.iter()
                        .find(|row| row["sessionId"] == session_id.as_str())
                        .filter(|row| row["_meta"]["yi"]["lastState"] == "idle")
                        .cloned()
                })
                .is_some())
        })
        .and_then(|()| second.request("2", "session/list", json!({})))?;
        let row = entry["result"]["sessions"]
            .as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|row| row["sessionId"] == session_id.as_str())
                    .cloned()
            })
            .ok_or("the session row")?;
        // Running then idle both happened unattended; the event stream between them is
        // dozens of frames and must not count.
        assert!(
            row["_meta"]["yi"]["unseen"]
                .as_u64()
                .is_some_and(|n| (1..=2).contains(&n)),
            "the missed transitions, not the event stream: {row}"
        );
        assert_eq!(
            row["title"], "fan this out",
            "the ledger names the session: {row}"
        );

        second.send(
            "3",
            "session/resume",
            json!({"sessionId": session_id, "cwd": dir.display().to_string(), "replayFrom": 0}),
        )?;
        let frames = second.read_until(|frame| frame["id"] == "3")?;
        let replays: Vec<&Value> = frames
            .iter()
            .filter(|frame| frame["params"]["update"]["sessionUpdate"] == "_yi/replay")
            .collect();
        assert_eq!(
            replays.len(),
            1,
            "one replay chunk before the response: {frames:?}"
        );
        let entries: Vec<yi_types::entry::Entry> =
            serde_json::from_value(replays[0]["params"]["update"]["entries"].clone())?;
        assert!(
            entries.len() >= 2,
            "user and assistant entries: {entries:?}"
        );
        let response = frames.last().ok_or("no response")?;
        assert_eq!(response["result"]["_meta"]["yi"]["name"], "fan this out");
        Ok(())
    })();

    let _cleanup = daemon.kill();
    let _reaped = daemon.wait();
    outcome
}

fn listed_row(
    client: &mut DaemonClient,
    session_id: &str,
) -> Result<Option<Value>, Box<dyn Error>> {
    let list = client.request("2", "session/list", json!({}))?;
    Ok(list["result"]["sessions"].as_array().and_then(|rows| {
        rows.iter()
            .find(|row| row["sessionId"] == session_id)
            .cloned()
    }))
}

/// The ledger outlives the daemon: a session prompted before a shutdown is listed by the
/// next daemon on the same socket with its name, root and unseen count, and idle.
#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn the_ledger_survives_a_daemon_restart() -> TestResult {
    let dir = Scratch::new("yi-serve-ledger")?;
    let (mut daemon, socket) = spawn_daemon(&dir)?;
    let outcome = (|| -> TestResult {
        let mut first = DaemonClient::connect(&socket)?;
        first.request("1", "initialize", json!({"protocolVersion": 2}))?;
        let session_id = new_session(&mut first, "2", &dir)?;
        first.send(
            "3",
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "keep me"}]}),
        )?;
        drop(first);
        let mut second = DaemonClient::connect(&socket)?;
        second.request("1", "initialize", json!({"protocolVersion": 2}))?;
        wait_until(Instant::now() + Duration::from_secs(20), || {
            Ok(listed_row(&mut second, &session_id)?
                .is_some_and(|row| row["_meta"]["yi"]["lastState"] == "idle"))
        })?;
        second.request("s", "_yi/shutdown", json!({}))?;
        wait_until(Instant::now() + Duration::from_secs(5), || {
            Ok(!socket.exists())
        })?;
        let _ = daemon.wait();

        let (restarted, socket) = spawn_daemon(&dir)?;
        daemon = restarted;
        let mut third = DaemonClient::connect(&socket)?;
        third.request("1", "initialize", json!({"protocolVersion": 2}))?;
        let row = listed_row(&mut third, &session_id)?
            .ok_or("the restarted daemon must list the session")?;
        assert_eq!(row["title"], "keep me", "the name survives: {row}");
        assert_eq!(
            row["cwd"],
            dir.display().to_string(),
            "the root survives: {row}"
        );
        assert_eq!(
            row["_meta"]["yi"]["lastState"], "idle",
            "no worker outlived the daemon: {row}"
        );
        assert!(
            row["_meta"]["yi"]["unseen"]
                .as_u64()
                .is_some_and(|n| n >= 1),
            "the unseen turn survives: {row}"
        );
        Ok(())
    })();
    let _ = daemon.kill();
    let _ = daemon.wait();
    outcome
}

/// `_yi/shutdown` answers, then the daemon exits, its socket goes, and its worker dies
/// with it — the console's double ctrl+c must leave no orphan behind.
#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn shutdown_stops_the_daemon_and_its_worker() -> TestResult {
    let dir = Scratch::new("yi-serve-shutdown")?;
    let root = dir.join("repo");
    std::fs::create_dir_all(&root)?;
    let (mut daemon, socket) = spawn_daemon(&dir)?;
    let outcome = (|| -> TestResult {
        let mut client = DaemonClient::connect(&socket)?;
        let init = client.request(
            "i",
            "initialize",
            json!({"protocolVersion": 2, "clientInfo": {"name": "t"}}),
        )?;
        // A strict client reads a missing `session` object as an agent with no sessions.
        if !init["result"]["capabilities"]["session"]["delete"].is_object() {
            return Err(format!("the daemon must advertise sessions: {init}").into());
        }
        new_session(&mut client, "n", &root)?;
        let reply = client.request("s", "_yi/shutdown", json!({}))?;
        if reply.get("result").is_none() {
            return Err(format!("shutdown must answer: {reply}").into());
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(_status) = daemon.try_wait()? {
                break;
            }
            if Instant::now() > deadline {
                return Err("the daemon must exit after _yi/shutdown".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if socket.exists() {
            return Err("the socket must be removed on shutdown".into());
        }
        #[expect(
            clippy::disallowed_methods,
            reason = "the worker is a grandchild the test can only see through the process table"
        )]
        let workers = Command::new("pgrep")
            .args(["-f", &format!("acp --cwd {}", root.display())])
            .output()?;
        if workers.status.success() {
            return Err("the worker must die with the daemon".into());
        }
        Ok(())
    })();
    let _ = daemon.kill();
    let _ = daemon.wait();
    outcome
}

fn git_in(dir: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the fixture repository is built with the real git the worker will claim a lane from"
    )]
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()?;
    if !output.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// D208: a worker speaks the moment it attaches, which is before its `session/new` response
/// creates the daemon's entry — so the first thing it says, where the session runs, was
/// dropped and the console painted the launch root for the whole session.
#[test]
fn the_daemon_delivers_the_workdir_the_worker_named_at_attach() -> TestResult {
    let dir = Scratch::new("yi-serve-workdir")?;
    let home = dir.home()?;
    let root = dir.join("project");
    std::fs::create_dir_all(&root)?;
    git_in(&root, &["init", "-q", "-b", "main"])?;
    git_in(
        &root,
        &[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "base",
        ],
    )?;
    let (mut daemon, socket) = spawn_daemon_in(&dir, Some(&home))?;
    let mut client = DaemonClient::connect(&socket)?;
    client.request("1", "initialize", json!({"protocolVersion": 2}))?;
    client.request_with_notifications(
        "2",
        "session/new",
        json!({"cwd": root.display().to_string()}),
    )?;
    // The lane is claimed after `session/new` answers, so the update follows the response.
    let frames =
        client.read_until(|frame| frame["params"]["update"]["sessionUpdate"] == "_yi/workdir")?;
    let workdir = frames
        .iter()
        .filter(|frame| frame["method"] == "session/update")
        .map(|frame| &frame["params"]["update"])
        .find(|update| update["sessionUpdate"] == "_yi/workdir")
        .ok_or_else(|| format!("no _yi/workdir reached the client: {frames:#?}"))?;
    assert!(
        workdir["cwd"]
            .as_str()
            .is_some_and(|cwd| cwd.contains(".yi/lanes/")),
        "the client is told the lane, not the launch root: {workdir}"
    );
    let _ = daemon.kill();
    Ok(())
}
