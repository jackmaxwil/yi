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
        Ok(Self {
            stream,
            lines: BufReader::new(reader).lines(),
        })
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
