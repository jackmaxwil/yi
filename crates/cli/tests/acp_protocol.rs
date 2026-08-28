use std::error::Error;
use std::io::{BufRead, BufReader, Lines, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

type TestResult = Result<(), Box<dyn Error>>;

fn temp_dir(tag: &str) -> Result<std::path::PathBuf, Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!("yi-acp-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

struct AcpClient {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
}

impl AcpClient {
    fn spawn(dir: &std::path::Path) -> Result<Self, Box<dyn Error>> {
        #[expect(
            clippy::disallowed_methods,
            reason = "the protocol contract is the spawned binary's stdio; tests must drive the real process"
        )]
        let mut child = Command::new(env!("CARGO_BIN_EXE_yi"))
            .args([
                "acp",
                "--model",
                "faux/faux-1",
                "--session-dir",
                &dir.join("sessions").display().to_string(),
                "--cwd",
                &dir.display().to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        Ok(Self {
            child,
            stdin,
            lines: BufReader::new(stdout).lines(),
        })
    }

    fn send(&mut self, frame: &Value) -> Result<(), Box<dyn Error>> {
        serde_json::to_writer(&mut self.stdin, frame)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn request(
        &mut self,
        id: &str,
        method: &str,
        params: Value,
    ) -> Result<Vec<Value>, Box<dyn Error>> {
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        self.read_until(|frame| frame["id"] == id)
    }

    /// Reads frames until `stop` matches; returns every frame read,
    /// the matching one last.
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

    fn finish(mut self) -> Result<(), Box<dyn Error>> {
        drop(self.stdin);
        self.child.wait()?;
        Ok(())
    }
}

fn updates_of<'a>(frames: &'a [Value], kind: &str) -> Vec<&'a Value> {
    frames
        .iter()
        .filter(|frame| {
            frame["method"] == "session/update"
                && frame["params"]["update"]["sessionUpdate"] == kind
        })
        .collect()
}

#[test]
fn v2_client_drives_a_session_end_to_end() -> TestResult {
    let dir = temp_dir("e2e")?;
    let mut client = AcpClient::spawn(&dir)?;

    let init = client.request("1", "initialize", json!({"protocolVersion": 2}))?;
    let init = init.last().ok_or("no initialize response")?;
    assert_eq!(init["result"]["protocolVersion"], 2);
    assert_eq!(init["result"]["info"]["name"], "yi");

    let new = client.request(
        "2",
        "session/new",
        json!({"cwd": dir.display().to_string()}),
    )?;
    let new = new.last().ok_or("no session/new response")?;
    let session_id = new["result"]["sessionId"]
        .as_str()
        .ok_or("missing sessionId")?
        .to_owned();
    assert!(
        new["result"]["configOptions"]
            .as_array()
            .is_some_and(|options| !options.is_empty()),
        "session/new must advertise config options (C8)"
    );

    client.send(&json!({
        "jsonrpc": "2.0",
        "id": "3",
        "method": "session/prompt",
        "params": {
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": "sanity"}],
        },
    }))?;
    let frames = client.read_until(|frame| {
        frame["params"]["update"]["sessionUpdate"] == "state_update"
            && frame["params"]["update"]["state"] == "idle"
    })?;
    let chunks = updates_of(&frames, "agent_message_chunk");
    let text: String = chunks
        .iter()
        .filter_map(|frame| frame["params"]["update"]["content"]["text"].as_str())
        .collect();
    assert!(
        text.contains("faux:"),
        "the faux reply must stream as agent_message_chunk updates: {text:?}"
    );
    assert!(
        !updates_of(&frames, "state_update").is_empty(),
        "running/idle state updates must frame the turn"
    );

    let list = client.request("4", "session/list", json!({}))?;
    let list = list.last().ok_or("no session/list response")?;
    let listed = list["result"]["sessions"]
        .as_array()
        .ok_or("sessions must be an array")?;
    assert!(
        listed
            .iter()
            .any(|entry| entry["sessionId"] == session_id.as_str()),
        "the new session must appear in session/list"
    );

    let close = client.request("5", "session/close", json!({"sessionId": session_id}))?;
    assert!(
        close
            .last()
            .is_some_and(|frame| frame["result"].is_object())
    );
    client.finish()?;

    let session_files = std::fs::read_dir(dir.join("sessions"))?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .flat_map(|entry| std::fs::read_dir(entry.path()).into_iter().flatten())
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .count();
    assert!(session_files >= 1, "the session must persist as v4 JSONL");
    Ok(())
}

#[test]
fn a_schedule_mutation_notifies_the_client() -> TestResult {
    let dir = temp_dir("heartbeat")?;
    let mut client = AcpClient::spawn(&dir)?;
    client.request("1", "initialize", json!({"protocolVersion": 2}))?;
    let new = client.request(
        "2",
        "session/new",
        json!({"cwd": dir.display().to_string()}),
    )?;
    let session_id = new
        .last()
        .and_then(|frame| frame["result"]["sessionId"].as_str())
        .ok_or("missing sessionId")?
        .to_owned();

    let frames = client.request(
        "3",
        "_yi/heartbeat",
        json!({"sessionId": session_id, "command": "--every 10s check on the build"}),
    )?;
    let changed = updates_of(&frames, "_yi/heartbeat_changed");
    assert!(
        !changed.is_empty(),
        "a schedule mutation must notify the client (C9): {frames:?}"
    );
    client.finish()
}

#[test]
fn setting_the_permission_mode_takes_effect_and_echoes_back() -> TestResult {
    let dir = temp_dir("mode")?;
    let mut client = AcpClient::spawn(&dir)?;
    client.request("1", "initialize", json!({"protocolVersion": 2}))?;
    let new = client.request(
        "2",
        "session/new",
        json!({"cwd": dir.display().to_string()}),
    )?;
    let new = new.last().ok_or("no session/new response")?;
    let session_id = new["result"]["sessionId"]
        .as_str()
        .ok_or("missing sessionId")?
        .to_owned();
    let mode_of = |options: &Value| -> Option<String> {
        options.as_array()?.iter().find_map(|option| {
            (option["configId"] == "mode")
                .then(|| option["kind"]["value"].as_str().map(str::to_owned))?
        })
    };
    assert_eq!(
        mode_of(&new["result"]["configOptions"]).as_deref(),
        Some("yolo"),
        "the session must advertise its permission mode (C8)"
    );

    let set = client.request(
        "3",
        "session/set_config_option",
        json!({"sessionId": session_id, "configId": "mode", "value": "ask"}),
    )?;
    let set = set.last().ok_or("no set_config_option response")?;
    assert_eq!(
        mode_of(&set["result"]["configOptions"]).as_deref(),
        Some("ask"),
        "the applied mode must echo back in the options: {set}"
    );

    let rejected = client.request(
        "4",
        "session/set_config_option",
        json!({"sessionId": session_id, "configId": "mode", "value": "reckless"}),
    )?;
    let rejected = rejected.last().ok_or("no response")?;
    assert!(
        rejected["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("ask|auto|yolo")),
        "an unknown mode must name the legal set: {rejected}"
    );
    client.finish()
}

#[test]
fn v1_client_gets_the_exact_mismatch_error() -> TestResult {
    let dir = temp_dir("v1")?;
    let mut client = AcpClient::spawn(&dir)?;
    let init = client.request("1", "initialize", json!({"protocolVersion": 1}))?;
    let init = init.last().ok_or("no initialize response")?;
    assert_eq!(
        init["error"]["message"],
        "Unsupported protocol version: this agent speaks ACP v2 or later"
    );
    client.finish()?;
    Ok(())
}

#[test]
fn resume_replays_the_stored_branch() -> TestResult {
    let dir = temp_dir("resume")?;
    let mut client = AcpClient::spawn(&dir)?;
    client.request("1", "initialize", json!({"protocolVersion": 2}))?;
    let new = client.request(
        "2",
        "session/new",
        json!({"cwd": dir.display().to_string()}),
    )?;
    let session_id = new
        .last()
        .and_then(|frame| frame["result"]["sessionId"].as_str())
        .ok_or("missing sessionId")?
        .to_owned();
    client.send(&json!({
        "jsonrpc": "2.0",
        "id": "3",
        "method": "session/prompt",
        "params": {"sessionId": session_id, "prompt": [{"type": "text", "text": "sanity"}]},
    }))?;
    client.read_until(|frame| {
        frame["params"]["update"]["sessionUpdate"] == "state_update"
            && frame["params"]["update"]["state"] == "idle"
    })?;
    client.request("4", "session/close", json!({"sessionId": session_id}))?;

    client.send(&json!({
        "jsonrpc": "2.0",
        "id": "5",
        "method": "session/resume",
        "params": {"sessionId": session_id, "replayFrom": {"type": "start"}},
    }))?;
    let frames = client.read_until(|frame| frame["id"] == "5")?;
    let users = updates_of(&frames, "user_message");
    assert!(
        users
            .iter()
            .any(|frame| { frame["params"]["update"]["content"][0]["text"] == "sanity" }),
        "resume with replayFrom must replay the stored user message (C6): {frames:?}"
    );
    client.finish()?;
    Ok(())
}
