use std::error::Error;
use std::io::{BufRead, BufReader, Lines, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

#[path = "support/acp_schema.rs"]
mod acp_schema;
use acp_schema::Schema;

type TestResult = Result<(), Box<dyn Error>>;

fn temp_dir(tag: &str) -> Result<Scratch, Box<dyn Error>> {
    let dir = Scratch::new(&format!("yi-acp-{tag}"))?;
    dir.home()?;
    Ok(dir)
}

struct AcpClient {
    child: Child,
    stdin: ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
    asked: std::collections::HashMap<String, String>,
    log: Vec<Value>,
}

impl AcpClient {
    fn spawn(dir: &std::path::Path) -> Result<Self, Box<dyn Error>> {
        Self::spawn_with(dir, &[])
    }

    fn spawn_with(dir: &std::path::Path, extra: &[&str]) -> Result<Self, Box<dyn Error>> {
        #[expect(
            clippy::disallowed_methods,
            reason = "the protocol contract is the spawned binary's stdio; tests must drive the real process"
        )]
        let mut child = Command::new(env!("CARGO_BIN_EXE_yi"))
            // Invariant: a harness never claims from the person's pool.
            .env("HOME", dir.join("home"))
            .args([
                "acp",
                "--model",
                "faux/faux-1",
                "--session-dir",
                &dir.join("sessions").display().to_string(),
                "--cwd",
                &dir.display().to_string(),
            ])
            .args(extra)
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
            asked: std::collections::HashMap::new(),
            log: Vec::new(),
        })
    }

    fn send(&mut self, frame: &Value) -> Result<(), Box<dyn Error>> {
        if let (Some(id), Some(method)) = (frame["id"].as_str(), frame["method"].as_str()) {
            self.asked.insert(id.to_owned(), method.to_owned());
        }
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
            self.log.push(frame.clone());
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
    let own = listed
        .iter()
        .find(|entry| entry["sessionId"] == session_id.as_str())
        .ok_or("the new session must appear in session/list")?;
    assert!(
        own["_meta"]["yi"]["createdAt"].is_u64(),
        "a listed session carries its birth for the sidebar's ages: {own}"
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
                .then(|| option["currentValue"].as_str().map(str::to_owned))?
        })
    };
    assert_eq!(
        mode_of(&new["result"]["configOptions"]).as_deref(),
        Some("auto"),
        "the session must advertise its permission mode (C8)"
    );

    let set = client.request(
        "3",
        "session/set_config_option",
        json!({"sessionId": session_id, "configId": "mode", "type": "id", "value": "ask"}),
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
        json!({"sessionId": session_id, "configId": "mode", "type": "id", "value": "reckless"}),
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

/// The daemon fans a worker's asks out to every attached client, so the worker never confirms
/// (plan section 3.7's fallback): a `_yi/plan` submit of `fuse_reset` is refused with no
/// `session/request_permission` raised, an actor in the frame is refused, the owner's own ops
/// apply, and nothing by a user reaches the journal.
#[test]
fn a_submit_over_the_worker_cannot_confirm_an_administrative_op() -> TestResult {
    let dir = temp_dir("plan-submit")?;
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
    let submit = |args: Value| json!({"sessionId": session_id, "action": "submit", "args": args});
    let opened = client.request(
        "3",
        "_yi/plan",
        submit(json!({"op": "init", "goal": "ship the seam", "todos": [{"label": "cut"}]})),
    )?;
    let opened = opened.last().ok_or("no init response")?;
    assert!(opened["result"]["plan"].is_string(), "{opened}");
    let mut claimed = submit(json!({"op": "fuse_reset"}));
    claimed["actor"] = json!("user://1");
    let claimed = client.request("4", "_yi/plan", claimed)?;
    let claimed = claimed.last().ok_or("no response")?;
    assert!(
        claimed["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("actor is not an argument")),
        "{claimed}"
    );
    let refused = client.request("5", "_yi/plan", submit(json!({"op": "fuse_reset"})))?;
    assert!(
        refused
            .iter()
            .all(|frame| frame["method"] != "session/request_permission"),
        "the worker raised an ask a peer could answer: {refused:?}"
    );
    let refused = refused.last().ok_or("no fuse_reset response")?;
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("confirmation")),
        "{refused}"
    );
    let mut recorded = String::new();
    for entry in std::fs::read_dir(dir.join(".yi/plans"))? {
        let path = entry?.path().join("ops.jsonl");
        if path.is_file() {
            recorded.push_str(&std::fs::read_to_string(path)?);
        }
    }
    assert!(
        recorded.contains(r#""op":"init""#)
            && !recorded.contains(r#""op":"fuse_reset""#)
            && !recorded.contains("user://"),
        "a user reached the journal over the socket: {recorded}"
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

/// `_yi/slash` runs the chat's slash verbs on the worker that holds the session, so a
/// console gets the same answers the solo TUI computes locally.
#[test]
fn slash_verbs_run_on_the_worker_and_unknown_ones_are_refused() -> TestResult {
    let dir = temp_dir("slash")?;
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
    let text_of = |frames: &[Value]| -> Option<String> {
        frames.last()?["result"]["text"].as_str().map(str::to_owned)
    };
    let mode = client.request(
        "3",
        "_yi/slash",
        json!({"sessionId": session_id, "line": "permissions"}),
    )?;
    assert!(
        text_of(&mode).is_some_and(|text| text.starts_with("permission mode: ")),
        "/permissions reads the broker: {mode:?}"
    );
    let listed = client.request(
        "4",
        "_yi/slash",
        json!({"sessionId": session_id, "line": "sessions"}),
    )?;
    assert!(
        text_of(&listed).is_some_and(|text| text.contains(&session_id)),
        "/sessions lists this root: {listed:?}"
    );
    let undone = client.request(
        "5u",
        "_yi/slash",
        json!({"sessionId": session_id, "line": "undo"}),
    )?;
    assert_eq!(
        text_of(&undone).as_deref(),
        Some("/undo: this session has taken no turn yet — `yi undo` restores an earlier session"),
        "/undo answers as the solo chat does: {undone:?}"
    );
    let unknown = client.request(
        "5",
        "_yi/slash",
        json!({"sessionId": session_id, "line": "dance"}),
    )?;
    assert!(
        unknown
            .last()
            .is_some_and(|frame| frame["error"].is_object()),
        "an unknown verb is an error, not a prompt: {unknown:?}"
    );
    let heartbeat = client.request(
        "6",
        "_yi/slash",
        json!({"sessionId": session_id, "line": "heartbeat every 10m watch the build"}),
    )?;
    assert!(
        text_of(&heartbeat).is_some(),
        "/heartbeat routes through the worker: {heartbeat:?}"
    );
    let changed = updates_of(&heartbeat, "_yi/heartbeat_changed");
    assert!(
        !changed.is_empty(),
        "/heartbeat over _yi/slash must still notify the client (C9): {heartbeat:?}"
    );
    client.finish()
}

fn new_faux_session(
    client: &mut AcpClient,
    dir: &std::path::Path,
) -> Result<String, Box<dyn Error>> {
    client.request("1", "initialize", json!({"protocolVersion": 2}))?;
    let new = client.request(
        "2",
        "session/new",
        json!({"cwd": dir.display().to_string()}),
    )?;
    new.last()
        .and_then(|frame| frame["result"]["sessionId"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| "missing sessionId".into())
}

fn prompt_until_idle(
    client: &mut AcpClient,
    id: &str,
    session_id: &str,
    text: &str,
) -> Result<Vec<Value>, Box<dyn Error>> {
    client.send(&json!({
        "jsonrpc": "2.0", "id": id, "method": "session/prompt",
        "params": {"sessionId": session_id, "prompt": [{"type": "text", "text": text}]},
    }))?;
    client.read_until(|frame| {
        frame["params"]["update"]["sessionUpdate"] == "state_update"
            && frame["params"]["update"]["state"] == "idle"
    })
}

fn faux_model() -> yi_types::model::Model {
    let zero = || serde_json::Number::from(0_u64);
    yi_types::model::Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: yi_types::model::ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 128_000,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn event_of(frame: &Value) -> Result<yi_types::event::AgentEvent, Box<dyn Error>> {
    Ok(serde_json::from_value(
        frame["params"]["update"]["event"].clone(),
    )?)
}

/// The `_yi/event` stream is the runtime's own events, verbatim and in order: every frame
/// decodes and re-encodes identically, `seq` counts from zero, the faux turn's shape is the
/// documented one, and solo's reducer fed the decoded stream lands where solo lands.
#[test]
fn yi_event_stream_is_lossless_for_a_faux_turn() -> TestResult {
    let dir = temp_dir("lossless")?;
    let mut client = AcpClient::spawn(&dir)?;
    let session_id = new_faux_session(&mut client, &dir)?;
    let frames = prompt_until_idle(&mut client, "3", &session_id, "sanity")?;
    let events = updates_of(&frames, "_yi/event");
    assert!(
        !events.is_empty(),
        "the turn must stream _yi/event frames: {frames:?}"
    );

    let mut decoded = Vec::new();
    for (n, frame) in events.iter().enumerate() {
        let update = &frame["params"]["update"];
        assert_eq!(
            update["seq"], n,
            "seq must count from zero in wire order: {update}"
        );
        assert!(
            update.get("childId").is_none(),
            "the parent stream has no childId: {update}"
        );
        let event = event_of(frame)?;
        assert_eq!(
            serde_json::to_value(&event)?,
            update["event"],
            "decoding then encoding must reproduce the wire bytes exactly"
        );
        decoded.push(event);
    }

    let kinds: Vec<String> = events
        .iter()
        .map(|frame| {
            let update = &frame["params"]["update"]["event"];
            match update["type"].as_str() {
                Some("message_update") => format!(
                    "message_update/{}",
                    update["assistantMessageEvent"]["type"]
                        .as_str()
                        .unwrap_or("?")
                ),
                Some("message_start") | Some("message_end") => format!(
                    "{}/{}",
                    update["type"].as_str().unwrap_or("?"),
                    update["message"]["role"].as_str().unwrap_or("?")
                ),
                other => other.unwrap_or("?").to_owned(),
            }
        })
        .collect();
    assert_eq!(
        kinds.first().map(String::as_str),
        Some("agent_start"),
        "a turn opens with agent_start: {kinds:?}"
    );
    assert_eq!(
        kinds.last().map(String::as_str),
        Some("agent_end"),
        "a turn closes with agent_end: {kinds:?}"
    );
    assert!(
        kinds.iter().any(|kind| kind == "message_start/user"),
        "the prompt itself is on the stream: {kinds:?}"
    );
    assert!(
        kinds.iter().any(|kind| kind == "message_update/text_delta"),
        "assistant deltas are on the stream: {kinds:?}"
    );
    assert!(
        kinds.iter().any(|kind| kind == "message_end/assistant"),
        "the assistant message end with its usage is on the stream: {kinds:?}"
    );

    // A delta is a delta (D145): the stream carries no snapshot, so the
    // reader folds the deltas itself and the text must arrive in order.
    let mut streaming: Option<yi_types::message::AgentMessage> = None;
    let mut folded: Vec<String> = Vec::new();
    for event in &decoded {
        match event {
            yi_types::event::AgentEvent::MessageStart {
                message: message @ yi_types::message::AgentMessage::Assistant { .. },
            } => streaming = Some(message.clone()),
            yi_types::event::AgentEvent::MessageUpdate {
                assistant_message_event,
            } => {
                if let Some(message) = streaming.as_mut() {
                    yi_types::event::apply(message, assistant_message_event);
                    if let yi_types::message::AgentMessage::Assistant { content, .. } = message {
                        folded.push(
                            content
                                .iter()
                                .filter_map(|block| match block {
                                    yi_types::message::Content::Text { text, .. } => {
                                        Some(text.as_str())
                                    }
                                    _ => None,
                                })
                                .collect::<String>(),
                        );
                    }
                }
            }
            _ => {}
        }
    }
    for pair in folded.windows(2) {
        assert!(
            pair[1].starts_with(&pair[0]),
            "each folded delta extends the last: {:?} then {:?}",
            pair[0],
            pair[1]
        );
    }
    let bytes_on_wire: usize = decoded
        .iter()
        .filter(|event| matches!(event, yi_types::event::AgentEvent::MessageUpdate { .. }))
        .map(|event| serde_json::to_string(event).map(|s| s.len()).unwrap_or(0))
        .sum();
    let final_len = folded.last().map(String::len).unwrap_or(0);
    assert!(
        bytes_on_wire < final_len * 8 + 4096,
        "the update stream is linear in the answer: {bytes_on_wire} bytes for {final_len} chars"
    );

    let options = yi_tui::TuiOptions {
        model: faux_model(),
        session_name: "lossless".to_owned(),
        cwd: dir.display().to_string(),
        lane: None,
        context_window: 200_000,
        session_dir: dir.display().to_string(),
        keys: Vec::new(),
        initial_prompt: None,
        pace: 0,
    };
    let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::Ansi16, true);
    let mut app = yi_tui::app::App::new(options, theme, yi_tui::keymap::default_keymap(), 80);
    for event in decoded {
        app.reduce_agent(event);
    }
    assert!(!app.is_running(), "solo's reducer must see the turn end");
    assert!(app.has_run(), "solo's reducer must see the turn start");
    let text: String = app
        .reflowed(200)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("sanity"),
        "the prompt reaches solo's history: {text:?}"
    );
    assert!(
        text.contains("faux:"),
        "the reply reaches solo's history: {text:?}"
    );
    client.finish()
}

/// A resume replays the branch verbatim as `_yi/replay` before its response, names the
/// session, and the rewind and steer verbs route to the worker.
#[test]
fn resume_replays_the_branch_verbatim_and_rewind_reloads_it() -> TestResult {
    let dir = temp_dir("replay")?;
    let mut client = AcpClient::spawn(&dir)?;
    let session_id = new_faux_session(&mut client, &dir)?;
    prompt_until_idle(&mut client, "3", &session_id, "fan this out")?;

    let frames = client.request(
        "4",
        "session/resume",
        json!({"sessionId": session_id, "replayFrom": 0}),
    )?;
    let replays = updates_of(&frames, "_yi/replay");
    assert_eq!(
        replays.len(),
        1,
        "one replay chunk for a short branch: {frames:?}"
    );
    let replay = &replays[0]["params"]["update"];
    let entries: Vec<yi_types::entry::Entry> = serde_json::from_value(replay["entries"].clone())?;
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            yi_types::entry::Entry::Message {
                message: yi_types::message::AgentMessage::User { .. },
                ..
            }
        )),
        "the user entry is on the replay: {entries:?}"
    );
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            yi_types::entry::Entry::Message {
                message: yi_types::message::AgentMessage::Assistant { .. },
                ..
            }
        )),
        "the assistant entry is on the replay: {entries:?}"
    );
    assert_eq!(replay["replayedTo"], entries.len());
    assert!(
        replay["leafId"].is_string(),
        "the branch leaf rides the replay: {replay}"
    );
    assert_eq!(replay["name"], "fan this out");
    assert!(
        replay["contextWindow"]
            .as_u64()
            .is_some_and(|size| size > 0)
    );
    let response = frames.last().ok_or("no resume response")?;
    assert_eq!(
        response["result"]["_meta"]["yi"]["name"], "fan this out",
        "the result names the session"
    );
    assert_eq!(
        response["result"]["_meta"]["yi"]["replayedTo"],
        entries.len()
    );
    let response_at = frames.len().saturating_sub(1);
    let replay_at = frames
        .iter()
        .position(|frame| frame["params"]["update"]["sessionUpdate"] == "_yi/replay")
        .ok_or("replay position")?;
    assert!(
        replay_at < response_at,
        "the replay lands before the response"
    );

    let steered = client.request(
        "5",
        "_yi/steer",
        json!({"sessionId": session_id, "text": "x"}),
    )?;
    assert!(
        steered
            .last()
            .is_some_and(|frame| frame["result"].is_object()),
        "_yi/steer answers with an empty result: {steered:?}"
    );

    let user_id = entries
        .iter()
        .find_map(|entry| match entry {
            yi_types::entry::Entry::Message {
                id,
                message: yi_types::message::AgentMessage::User { .. },
                ..
            } => Some(id.clone()),
            _ => None,
        })
        .ok_or("user entry id")?;
    let rewound = client.request(
        "6",
        "_yi/rewind",
        json!({"sessionId": session_id, "entryId": user_id}),
    )?;
    let response = rewound.last().ok_or("no rewind response")?;
    assert_eq!(
        response["result"]["unsent"], "fan this out",
        "landing on a user turn hands its text back"
    );
    assert_ne!(
        response["result"]["leafId"], user_id,
        "the leaf moved off the user turn"
    );
    let after = updates_of(&rewound, "_yi/replay");
    assert_eq!(after.len(), 1, "a rewind re-sends the branch: {rewound:?}");
    assert_eq!(
        after[0]["params"]["update"]["leafId"], response["result"]["leafId"],
        "the replay after a rewind names the same leaf the result does"
    );
    client.finish()
}

/// `replayUpdates: false` drops the standard updates a yi client would decode twice; the
/// default resume keeps them for generic ACP clients.
#[test]
fn resume_can_opt_out_of_the_standard_replay_updates() -> TestResult {
    let dir = temp_dir("replay-opt-out")?;
    let mut client = AcpClient::spawn(&dir)?;
    let session_id = new_faux_session(&mut client, &dir)?;
    prompt_until_idle(&mut client, "3", &session_id, "just the extension")?;

    let lean = client.request(
        "4",
        "session/resume",
        json!({"sessionId": session_id, "replayFrom": 0, "_meta": {"yi": {"replayUpdates": false}}}),
    )?;
    let standard = ["user_message", "agent_message"];
    assert!(
        standard
            .iter()
            .all(|kind| updates_of(&lean, kind).is_empty()),
        "no standard replay updates when opted out: {lean:?}"
    );
    let replays = updates_of(&lean, "_yi/replay");
    assert_eq!(replays.len(), 1, "the extension still replays: {lean:?}");
    let entries = replays[0]["params"]["update"]["entries"]
        .as_array()
        .map_or(0, Vec::len);
    assert!(entries > 0, "the replay carries the branch");
    let response = lean.last().ok_or("no resume response")?;
    assert_eq!(response["result"]["_meta"]["yi"]["replayedTo"], entries);

    let full = client.request(
        "5",
        "session/resume",
        json!({"sessionId": session_id, "replayFrom": 0}),
    )?;
    assert!(
        standard
            .iter()
            .all(|kind| !updates_of(&full, kind).is_empty()),
        "the default resume still sends the standard updates: {full:?}"
    );
    client.finish()
}

fn git_in(dir: &std::path::Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the fixture is a real repository"
    )]
    let output = Command::new("git").current_dir(dir).args(args).output()?;
    if !output.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// One worker serves a root; two sessions on it are two lanes under two branches, because
/// the claim carries the store id rather than the worker's pid.
#[test]
fn two_sessions_on_one_worker_hold_two_lanes() -> Result<(), Box<dyn Error>> {
    let dir = temp_dir("two-lanes")?;
    git_in(&dir, &["init", "-q", "-b", "main"])?;
    git_in(
        &dir,
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
    let mut client = AcpClient::spawn(&dir)?;
    client.request("1", "initialize", json!({"protocolVersion": 2}))?;
    let mut ids = Vec::new();
    let is_workdir = |frame: &Value| frame["params"]["update"]["sessionUpdate"] == "_yi/workdir";
    let mut workdirs = 0;
    for id in ["2", "3"] {
        let frames = client.request(
            id,
            "session/new",
            json!({"cwd": dir.display().to_string(), "mcpServers": []}),
        )?;
        workdirs += frames.iter().filter(|frame| is_workdir(frame)).count();
        let last = frames.last().ok_or("no response")?;
        assert!(
            last.get("error").is_none(),
            "session/new {id} failed: {last}"
        );
        ids.push(
            last["result"]["sessionId"]
                .as_str()
                .ok_or("no sessionId")?
                .to_owned(),
        );
    }
    // Each lane is claimed after its `session/new` answers and named once it is held.
    while workdirs < 2 {
        client.read_until(is_workdir)?;
        workdirs += 1;
    }
    let worktrees = git_in(&dir, &["worktree", "list", "--porcelain"])?;
    for id in &ids {
        assert!(
            worktrees.contains(&format!("branch refs/heads/yi/{id}")),
            "session {id} has its own lane branch:\n{worktrees}"
        );
    }
    client.finish()?;
    Ok(())
}

/// D208: the console painted the root it launched in for a session that ran in a lane, so the
/// status row named the wrong tree and branch for the whole session.
#[test]
fn attach_names_the_lane_path_not_the_launch_root() -> Result<(), Box<dyn Error>> {
    let dir = temp_dir("workdir-update")?;
    git_in(&dir, &["init", "-q", "-b", "main"])?;
    git_in(
        &dir,
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
    let mut client = AcpClient::spawn(&dir)?;
    client.request("1", "initialize", json!({"protocolVersion": 2}))?;
    // The lane is claimed after `session/new` answers, so the update follows the response.
    client.request(
        "2",
        "session/new",
        json!({"cwd": dir.display().to_string(), "mcpServers": []}),
    )?;
    let frames =
        client.read_until(|frame| frame["params"]["update"]["sessionUpdate"] == "_yi/workdir")?;
    let workdir = frames
        .iter()
        .filter(|frame| frame["method"] == "session/update")
        .map(|frame| &frame["params"]["update"])
        .find(|update| update["sessionUpdate"] == "_yi/workdir")
        .ok_or_else(|| format!("no _yi/workdir update in {frames:#?}"))?;
    let cwd = workdir["cwd"].as_str().unwrap_or_default().to_owned();
    assert!(
        cwd.contains(".yi/lanes/"),
        "the workdir update names the lane, not the launch root: {cwd}"
    );
    assert!(
        workdir["lane"]
            .as_str()
            .is_some_and(|lane| lane.contains("⎇ lane")),
        "the update carries the row label: {workdir}"
    );
    client.finish()?;
    Ok(())
}

/// Dies with the Tape pane empty forever: the worker had a `_yi/tape` handler its request
/// dispatch never reached, so every ask came back as an unknown method.
#[test]
fn a_tape_request_reaches_the_worker_and_marks_the_typed_turn() -> TestResult {
    let dir = temp_dir("tape")?;
    let mut client = AcpClient::spawn(&dir)?;
    let session_id = new_faux_session(&mut client, &dir)?;
    prompt_until_idle(&mut client, "3", &session_id, "time me")?;
    let frames = client.request("4", "_yi/tape", json!({"sessionId": session_id}))?;
    let response = frames.last().ok_or("no tape response")?;
    assert!(response["error"].is_null(), "{response}");
    let marks = response["result"]["marks"]
        .as_array()
        .ok_or("no marks in the tape")?;
    assert!(
        marks
            .iter()
            .any(|mark| mark["kind"] == "user" && mark["label"] == "time me"),
        "{response}"
    );
    client.finish()
}

/// Every frame the client read, judged by the pinned upstream schema: responses by the method
/// they answer, updates and asks by their own def, `_yi/*` methods left to Yi.
fn strict_breaks(client: &AcpClient) -> Result<Vec<String>, Box<dyn Error>> {
    let schema = Schema::pinned()?;
    let mut broken = Vec::new();
    for frame in &client.log {
        let verdict = match frame["method"].as_str() {
            Some("session/update") => schema.check("UpdateSessionNotification", &frame["params"]),
            Some("session/request_permission") => {
                schema.check("RequestPermissionRequest", &frame["params"])
            }
            Some(_) => Ok(()),
            None => {
                let method = frame["id"]
                    .as_str()
                    .and_then(|id| client.asked.get(id))
                    .map_or("", String::as_str);
                if method.starts_with('_') {
                    Ok(())
                } else if let Some(error) = frame.get("error") {
                    schema.check("Error", error)
                } else {
                    let def = schema
                        .def_for(method, "Response")
                        .ok_or_else(|| format!("no response def for {method:?}"))?;
                    schema.check(def, &frame["result"])
                }
            }
        };
        if let Err(error) = verdict {
            broken.push(format!("{error}\n    in {frame}"));
        }
    }
    Ok(broken)
}

fn faux_script(dir: &std::path::Path, turns: &[Value]) -> Result<String, Box<dyn Error>> {
    use yi_runtime::faux::{faux_assistant_message, faux_text, faux_tool_call};
    use yi_types::message::StopReason;
    let lines = turns
        .iter()
        .map(|turn| {
            let message = match turn["tool"].as_str() {
                Some(tool) => faux_assistant_message(
                    vec![faux_tool_call(
                        turn["id"].as_str().unwrap_or("c1"),
                        tool,
                        turn["args"].as_object().cloned().unwrap_or_default(),
                    )],
                    StopReason::ToolUse,
                ),
                None => faux_assistant_message(
                    vec![faux_text(turn["text"].as_str().unwrap_or("done"))],
                    StopReason::Stop,
                ),
            };
            serde_json::to_string(&message)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let path = dir.join("script.jsonl");
    std::fs::write(&path, lines.join("\n"))?;
    Ok(path.display().to_string())
}

/// Reads until `stop`, answering every permission ask with `allow_once` on the way.
fn read_allowing(
    client: &mut AcpClient,
    stop: impl Fn(&Value) -> bool,
) -> Result<Vec<Value>, Box<dyn Error>> {
    let mut seen = Vec::new();
    loop {
        let frames = client
            .read_until(|frame| stop(frame) || frame["method"] == "session/request_permission")?;
        let last = frames.last().cloned().unwrap_or(Value::Null);
        seen.extend(frames);
        if stop(&last) {
            return Ok(seen);
        }
        client.send(&json!({
            "jsonrpc": "2.0", "id": last["id"],
            "result": {"outcome": {"outcome": "selected", "optionId": "allow_once"}},
        }))?;
    }
}

fn is_idle(frame: &Value) -> bool {
    frame["params"]["update"]["sessionUpdate"] == "state_update"
        && frame["params"]["update"]["state"] == "idle"
}

fn prompt(client: &mut AcpClient, id: &str, session_id: &str, blocks: Value) -> TestResult {
    client.send(&json!({
        "jsonrpc": "2.0", "id": id, "method": "session/prompt",
        "params": {"sessionId": session_id, "prompt": blocks},
    }))?;
    Ok(())
}

fn strict_session(client: &mut AcpClient, dir: &std::path::Path) -> Result<String, Box<dyn Error>> {
    client.request(
        "init",
        "initialize",
        json!({"protocolVersion": 2, "info": {"name": "strict-client", "version": "0"}}),
    )?;
    let new = client.request(
        "new",
        "session/new",
        json!({"cwd": dir.display().to_string()}),
    )?;
    new.last()
        .and_then(|frame| frame["result"]["sessionId"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| "missing sessionId".into())
}

/// Without `capabilities.session` a strict client reads the agent as sessionless and never
/// calls `session/new`; `session.delete` is what lets it offer the delete Yi serves.
#[test]
fn a_strict_client_finds_sessions_advertised_at_initialize() -> TestResult {
    let dir = temp_dir("strict-init")?;
    let mut client = AcpClient::spawn(&dir)?;
    let init = client.request("1", "initialize", json!({"protocolVersion": 2}))?;
    let capabilities = &init.last().ok_or("no initialize response")?["result"]["capabilities"];
    assert!(
        capabilities["session"].is_object(),
        "a strict client calls no session/* method: {capabilities}"
    );
    assert!(
        capabilities["session"]["delete"].is_object(),
        "a strict client hides delete: {capabilities}"
    );
    client.finish()
}

/// The whole surface a strict client touches, once, with an edit approved on the way and a
/// turn the provider fails, judged frame by frame against the pinned schema.
#[test]
fn a_strict_client_accepts_every_frame_of_a_faux_session() -> TestResult {
    let dir = temp_dir("strict-all")?;
    let script = faux_script(
        &dir,
        &[
            json!({"tool": "write", "args": {"path": "note.txt", "content": "tidy\n"}}),
            json!({"text": "wrote the note"}),
        ],
    )?;
    let mut client = AcpClient::spawn_with(&dir, &["--faux", &script])?;
    let session_id = strict_session(&mut client, &dir)?;
    client.request(
        "mode",
        "session/set_config_option",
        json!({"sessionId": session_id, "configId": "mode", "type": "id", "value": "ask"}),
    )?;
    prompt(
        &mut client,
        "p1",
        &session_id,
        json!([{"type": "text", "text": "tidy the note"}]),
    )?;
    read_allowing(&mut client, is_idle)?;
    prompt(
        &mut client,
        "p2",
        &session_id,
        json!([{"type": "text", "text": "and again"}]),
    )?;
    read_allowing(&mut client, is_idle)?;
    client.request("list", "session/list", json!({}))?;
    client.request("close", "session/close", json!({"sessionId": session_id}))?;
    client.request(
        "resume",
        "session/resume",
        json!({"sessionId": session_id, "cwd": dir.display().to_string(), "replayFrom": {"type": "start"}}),
    )?;
    client.request("delete", "session/delete", json!({"sessionId": session_id}))?;
    let broken = strict_breaks(&client)?;
    client.finish()?;
    assert!(
        broken.is_empty(),
        "a strict alpha.5 client rejects {} frame(s):\n{}",
        broken.len(),
        broken.join("\n")
    );
    Ok(())
}

/// A busy session queues the second prompt; each response must name the `user_message` that
/// its own text was inserted as, and arrive only once that message exists.
#[test]
fn the_prompt_response_names_the_user_message_it_inserted() -> TestResult {
    let dir = temp_dir("strict-ids")?;
    let script = faux_script(
        &dir,
        &[
            json!({"tool": "bash", "args": {"command": "sleep 1"}}),
            json!({"text": "slept"}),
            json!({"text": "second answer"}),
        ],
    )?;
    let mut client = AcpClient::spawn_with(&dir, &["--faux", &script])?;
    let session_id = strict_session(&mut client, &dir)?;
    prompt(
        &mut client,
        "first",
        &session_id,
        json!([{"type": "text", "text": "sleep a bit"}]),
    )?;
    prompt(
        &mut client,
        "second",
        &session_id,
        json!([{"type": "text", "text": "then this"}]),
    )?;
    let mut frames = read_allowing(&mut client, |frame| {
        frame["id"] == "second" && frame.get("method").is_none()
    })?;
    frames.extend(read_allowing(&mut client, is_idle)?);
    for (request, text) in [("first", "sleep a bit"), ("second", "then this")] {
        let answer = frames
            .iter()
            .position(|frame| frame["id"] == request && frame.get("method").is_none())
            .ok_or(format!("no response to {request}"))?;
        let message_id = frames[answer]["result"]["messageId"]
            .as_str()
            .ok_or(format!("{request}: no messageId in {}", frames[answer]))?;
        let inserted = frames[..answer].iter().find(|frame| {
            frame["params"]["update"]["sessionUpdate"] == "user_message"
                && frame["params"]["update"]["content"][0]["text"] == text
        });
        assert_eq!(
            inserted.map(|frame| &frame["params"]["update"]["messageId"]),
            Some(&json!(message_id)),
            "{request}: the response must follow the user_message it names"
        );
    }
    client.finish()
}

/// A prompt still queued when its session closes was never inserted, so it must not be
/// answered with an id that no message will ever carry.
#[test]
fn a_prompt_dropped_before_insertion_is_answered_cancelled() -> TestResult {
    let dir = temp_dir("strict-drop")?;
    let script = faux_script(
        &dir,
        &[
            json!({"tool": "bash", "args": {"command": "sleep 2"}}),
            json!({"text": "slept"}),
        ],
    )?;
    let mut client = AcpClient::spawn_with(&dir, &["--faux", &script])?;
    let session_id = strict_session(&mut client, &dir)?;
    prompt(
        &mut client,
        "first",
        &session_id,
        json!([{"type": "text", "text": "sleep a bit"}]),
    )?;
    read_allowing(&mut client, |frame| {
        frame["id"] == "first" && frame.get("method").is_none()
    })?;
    prompt(
        &mut client,
        "queued",
        &session_id,
        json!([{"type": "text", "text": "never runs"}]),
    )?;
    client.send(&json!({"jsonrpc": "2.0", "id": "close", "method": "session/close", "params": {"sessionId": session_id}}))?;
    let frames = read_allowing(&mut client, |frame| {
        frame["id"] == "queued" && frame.get("method").is_none()
    })?;
    let answer = frames.last().ok_or("no answer to the queued prompt")?;
    assert_eq!(
        answer["error"]["code"], -32800,
        "a dropped prompt is a cancelled request, never a messageId: {answer}"
    );
    client.finish()
}

#[test]
fn an_edit_approval_shows_its_diff_to_a_strict_client() -> TestResult {
    let dir = temp_dir("strict-diff")?;
    let script = faux_script(
        &dir,
        &[
            json!({"tool": "write", "args": {"path": "note.txt", "content": "tidy\n"}}),
            json!({"text": "wrote the note"}),
        ],
    )?;
    let mut client = AcpClient::spawn_with(&dir, &["--faux", &script])?;
    let session_id = strict_session(&mut client, &dir)?;
    client.request(
        "mode",
        "session/set_config_option",
        json!({"sessionId": session_id, "configId": "mode", "type": "id", "value": "ask"}),
    )?;
    prompt(
        &mut client,
        "p",
        &session_id,
        json!([{"type": "text", "text": "tidy the note"}]),
    )?;
    let frames = client.read_until(|frame| frame["method"] == "session/request_permission")?;
    let ask = &frames.last().ok_or("no ask")?["params"];
    let subject = &ask["subject"];
    assert_eq!(
        subject["type"], "tool_call",
        "the diff has no subject: {ask}"
    );
    let diff = &subject["toolCall"]["content"][0];
    assert_eq!(diff["type"], "diff", "{ask}");
    assert_eq!(diff["patch"]["format"], "git_patch", "{ask}");
    assert!(
        diff["patch"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("+tidy")),
        "{ask}"
    );
    let change = &diff["changes"][0];
    assert_eq!(change["operation"], "add", "note.txt did not exist: {ask}");
    assert!(
        change["path"]
            .as_str()
            .is_some_and(|path| path.starts_with('/') && path.ends_with("note.txt")),
        "a change path is absolute: {ask}"
    );
    assert!(ask.get("content").is_none(), "no custom root field: {ask}");
    let decoded: yi_types::acp::AcpPermissionParams = serde_json::from_value(ask.clone())?;
    assert!(
        matches!(
            decoded.subject,
            Some(yi_types::acp::AcpPermissionSubject::ToolCall { .. })
        ),
        "the console's own decoder must read the worker's ask: {decoded:?}"
    );
    client.send(&json!({
        "jsonrpc": "2.0", "id": frames.last().ok_or("no ask")?["id"],
        "result": {"outcome": {"outcome": "selected", "optionId": "allow_once"}},
    }))?;
    read_allowing(&mut client, is_idle)?;
    client.finish()
}

#[test]
fn a_resource_link_in_a_prompt_reaches_the_model() -> TestResult {
    let dir = temp_dir("strict-link")?;
    let mut client = AcpClient::spawn(&dir)?;
    let session_id = strict_session(&mut client, &dir)?;
    let link = "file:///repo/src/notes.md";
    prompt(
        &mut client,
        "p",
        &session_id,
        json!([
            {"type": "text", "text": "read"},
            {"type": "resource_link", "uri": link, "name": "notes.md"},
        ]),
    )?;
    let frames = read_allowing(&mut client, is_idle)?;
    let inserted = updates_of(&frames, "user_message");
    let text = inserted
        .first()
        .and_then(|frame| frame["params"]["update"]["content"][0]["text"].as_str())
        .ok_or("no user_message")?;
    assert_eq!(
        text,
        format!("read {link}"),
        "the linked file must reach the prompt as its own word"
    );
    client.finish()
}

fn tool_end(name: &str, details: Value, is_error: bool) -> yi_types::event::AgentEvent {
    yi_types::event::AgentEvent::ToolExecutionEnd {
        tool_call_id: "t1".to_owned(),
        tool_name: name.to_owned(),
        result: yi_types::event::ToolResult {
            content: vec![yi_types::message::Content::Text {
                text: "Operation aborted".to_owned(),
                text_signature: None,
            }],
            details,
            usage: None,
            added_tool_names: None,
            terminate: None,
        },
        is_error,
    }
}

/// Every update one event maps to, as the wire carries it, judged against the pinned schema.
fn mapped(event: &yi_types::event::AgentEvent) -> Result<Vec<Value>, Box<dyn Error>> {
    let schema = Schema::pinned()?;
    let mut ids = yi_acp::update::IdMap::new(1000);
    let mut out = Vec::new();
    for update in yi_acp::update::to_updates(event, &mut ids) {
        let frame = yi_acp::update::update_notification("s1", update);
        schema.check("UpdateSessionNotification", &frame["params"])?;
        out.push(frame["params"]["update"].clone());
    }
    Ok(out)
}

#[test]
fn a_cancelled_tool_reports_cancelled_not_failed() -> TestResult {
    let before_it_ran = mapped(&tool_end("write", json!({"errorKind": "aborted"}), true))?;
    let mid_command = mapped(&tool_end(
        "bash",
        json!({"exitCode": 0, "cancelled": true}),
        true,
    ))?;
    for updates in [before_it_ran, mid_command] {
        let status = updates
            .iter()
            .find(|update| update["sessionUpdate"] == "tool_call_update")
            .map(|update| update["status"].clone());
        assert_eq!(
            status,
            Some(json!("cancelled")),
            "a stop drew as a failure: {updates:?}"
        );
    }
    Ok(())
}

#[test]
fn a_negative_exit_code_never_reaches_the_wire() -> TestResult {
    let updates = mapped(&tool_end("bash", json!({"exitCode": -1}), true))?;
    let terminal = updates
        .iter()
        .find(|update| update["sessionUpdate"] == "terminal_update")
        .ok_or("no terminal_update")?;
    assert!(
        terminal["exitStatus"].get("exitCode").is_none(),
        "exitCode is uint32 on the wire: {terminal}"
    );
    Ok(())
}

#[test]
fn a_failed_turn_ends_on_an_extension_stop_reason() -> TestResult {
    use yi_types::event::{AgentEvent, AssistantMessageEvent};
    use yi_types::message::StopReason;
    let mut ids = yi_acp::update::IdMap::new(1000);
    let error = yi_runtime::faux::faux_assistant_message(Vec::new(), StopReason::Error);
    yi_acp::update::to_updates(
        &AgentEvent::MessageUpdate {
            assistant_message_event: AssistantMessageEvent::Error {
                reason: StopReason::Error,
                error,
            },
        },
        &mut ids,
    );
    let end = yi_acp::update::to_updates(
        &AgentEvent::AgentEnd {
            messages: Vec::new(),
        },
        &mut ids,
    );
    let schema = Schema::pinned()?;
    for update in end {
        let frame = yi_acp::update::update_notification("s1", update);
        schema.check("UpdateSessionNotification", &frame["params"])?;
    }
    Ok(())
}

/// The console decodes a peer's updates with these types; a value outside Yi's own vocabulary
/// must still decode as the standard kind, or the card silently stops updating.
#[test]
fn a_tool_status_yi_does_not_know_still_updates_the_card() -> TestResult {
    let frame = json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "t1",
        "name": "grep", "kind": "think", "status": "_paused",
    });
    let decoded: yi_types::acp::AcpSessionUpdate = serde_json::from_value(frame.clone())?;
    assert!(
        !matches!(decoded, yi_types::acp::AcpSessionUpdate::Extension(_)),
        "a standard update fell through to the extension carrier: {decoded:?}"
    );
    assert_eq!(
        serde_json::to_value(&decoded)?,
        frame,
        "the unknown values re-emit verbatim"
    );
    Ok(())
}

#[test]
fn an_ask_offering_reject_always_reaches_the_console() -> TestResult {
    let params = json!({
        "sessionId": "s1",
        "title": "run rm -rf target",
        "options": [
            {"optionId": "a", "name": "Allow once", "kind": "allow_once"},
            {"optionId": "n", "name": "Never", "kind": "reject_always"},
        ],
    });
    let decoded: yi_types::acp::AcpPermissionParams = serde_json::from_value(params)?;
    assert_eq!(decoded.options.len(), 2, "{decoded:?}");
    Ok(())
}
