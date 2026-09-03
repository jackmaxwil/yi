use std::error::Error;
use std::io::{BufRead, BufReader, Lines, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use yi_types::schedule::ScheduleState;

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
}

fn spawn_daemon(dir: &Path) -> Result<(Child, PathBuf), Box<dyn Error>> {
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
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() {
        if Instant::now() > deadline {
            return Err("daemon socket never appeared".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok((child, socket))
}

fn rlm_dirs(sessions: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let mut dirs = Vec::new();
    if !sessions.is_dir() {
        return Ok(dirs);
    }
    for entry in std::fs::read_dir(sessions)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with("rlm-") {
            dirs.push(entry.path());
        }
    }
    Ok(dirs)
}

fn ledger_path(rlm: &Path) -> PathBuf {
    rlm.join("scheduled-jobs.json")
}

fn any_heartbeat_dispatched(sessions: &Path) -> Result<bool, Box<dyn Error>> {
    Ok(rlm_dirs(sessions)?.into_iter().any(|dir| {
        std::fs::read_to_string(ledger_path(&dir)).is_ok_and(|contents| {
            contents.contains("\"dispatches\":[{") || contents.contains("\"runCount\"")
        })
    }))
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
    let dir = std::env::temp_dir().join(format!("yi-serve-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
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
        assert_eq!(
            resume["result"]["sessionId"], session_id,
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
    let dir = std::env::temp_dir().join(format!("yi-serve-roots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
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
        let dispatched = std::fs::read_dir(dir.join("sessions"))?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("rlm-"))
            .filter_map(|entry| {
                std::fs::read_to_string(entry.path().join("scheduled-jobs.json")).ok()
            })
            .filter(|contents| {
                contents.contains("\"dispatches\":[{") || contents.contains("\"runCount\"")
            })
            .count();
        assert_eq!(
            dispatched, 2,
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
                entry["name"], "sanity",
                "the ledger names a session from its first prompt: {entry}"
            );
            let resume =
                reconnected.request("3", "session/resume", json!({"sessionId": session}))?;
            assert_eq!(
                resume["result"]["sessionId"],
                session.as_str(),
                "resume must route to the surviving worker: {resume}"
            );
        }
        Ok(())
    })();

    let _cleanup = daemon.kill();
    let _reaped = daemon.wait();
    let _ = std::fs::remove_dir_all(&dir);
    outcome
}

/// Two roots spawn two workers. Two ACP sessions on a root share one
/// `rlm-{pid}` ledger and one timer; each session keeps its own heartbeat.
/// The failure is a sibling worker whose jobs never run, a torn
/// `scheduled-jobs.json`, or the second session cancelling the first.
#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn workers_do_not_lose_heartbeats_or_tear_the_job_ledger() -> TestResult {
    let dir = std::env::temp_dir().join(format!("yi-serve-g2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
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
            assert_eq!(
                resume["result"]["sessionId"],
                session_id.as_str(),
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
    let dirs = rlm_dirs(sessions)?;
    let mut out = format!("rlm dirs: {}\n", dirs.len());
    for dir in dirs {
        let path = ledger_path(&dir);
        let body = std::fs::read_to_string(&path).unwrap_or_else(|error| format!("read: {error}"));
        out.push_str(&format!("{}:\n{body}\n", path.display()));
    }
    Ok(out)
}

fn ledgers_are_intact(sessions: &Path) -> Result<bool, Box<dyn Error>> {
    let dirs = rlm_dirs(sessions)?;
    if dirs.len() != WORKERS {
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
        if state.jobs.iter().filter(|job| job.run_count >= 1).count() != SESSIONS_PER_WORKER {
            return Ok(false);
        }
        let mut ids = std::collections::BTreeSet::new();
        for job in &state.jobs {
            if job.run_count >= 1 {
                ids.insert(job.session_id.clone());
            }
        }
        if ids.len() != SESSIONS_PER_WORKER {
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
    let dir = std::env::temp_dir().join(format!("yi-fanout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
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
