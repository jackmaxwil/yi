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
    let own = listed
        .iter()
        .find(|entry| entry["sessionId"] == session_id.as_str())
        .ok_or("the new session must appear in session/list")?;
    assert!(
        own["createdAt"].is_u64(),
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
                .then(|| option["kind"]["value"].as_str().map(str::to_owned))?
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

    let snapshots: Vec<String> = decoded
        .iter()
        .filter_map(|event| match event {
            yi_types::event::AgentEvent::MessageUpdate {
                message: yi_types::message::AgentMessage::Assistant { content, .. },
                ..
            } => Some(
                content
                    .iter()
                    .filter_map(|block| match block {
                        yi_types::message::Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    for pair in snapshots.windows(2) {
        assert!(
            pair[1].starts_with(&pair[0]),
            "each snapshot extends the last: {:?} then {:?}",
            pair[0],
            pair[1]
        );
    }

    let options = yi_tui::TuiOptions {
        model: faux_model(),
        session_name: "lossless".to_owned(),
        cwd: dir.display().to_string(),
        context_window: 200_000,
        session_dir: dir.display().to_string(),
        keys: Vec::new(),
        initial_prompt: None,
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
        response["result"]["name"], "fan this out",
        "the result names the session"
    );
    assert_eq!(response["result"]["replayedTo"], entries.len());
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
