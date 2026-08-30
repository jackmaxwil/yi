use std::error::Error;
use std::io::{BufRead, BufReader, Lines, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

struct DaemonClient {
    stream: UnixStream,
    lines: Lines<BufReader<UnixStream>>,
}

impl DaemonClient {
    fn connect(socket: &std::path::Path) -> Result<Self, Box<dyn Error>> {
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

fn spawn_daemon(dir: &std::path::Path) -> Result<(Child, std::path::PathBuf), Box<dyn Error>> {
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
        let session_id = new["result"]["sessionId"]
            .as_str()
            .ok_or("missing sessionId")?
            .to_owned();

        let heartbeat = client.request(
            "3",
            "_yi/heartbeat",
            json!({
                "sessionId": session_id,
                "command": "--every 10s check on the build",
            }),
        )?;
        assert!(
            heartbeat["result"]["text"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "the heartbeat surface must acknowledge: {heartbeat}"
        );
        drop(client);

        // The client is gone; the worker's scheduler must keep firing.
        std::thread::sleep(Duration::from_secs(13));

        let dispatched = std::fs::read_dir(dir.join("sessions"))?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("rlm-"))
            .filter_map(|entry| {
                std::fs::read_to_string(entry.path().join("scheduled-jobs.json")).ok()
            })
            .any(|contents| {
                contents.contains("\"dispatches\":[{") || contents.contains("\"runCount\"")
            });
        assert!(
            dispatched,
            "the heartbeat must have dispatched while no client was attached"
        );

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
    Ok(new["result"]["sessionId"]
        .as_str()
        .ok_or("missing sessionId")?
        .to_owned())
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
            assert!(
                listed
                    .iter()
                    .any(|entry| entry["sessionId"] == session.as_str()),
                "the supervisor must still know {session} after reconnect: {list}"
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
