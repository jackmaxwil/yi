use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::Value;

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

fn temp_dir(tag: &str) -> Result<Scratch, Box<dyn Error>> {
    Ok(Scratch::new(&format!("yi-rpc-{tag}"))?)
}

fn run_rpc(dir: &std::path::Path, commands: &[Value]) -> Result<Vec<Value>, Box<dyn Error>> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the protocol contract is the spawned binary's stdio; tests must drive the real process"
    )]
    let mut child = Command::new(env!("CARGO_BIN_EXE_yi"))
        .args([
            "rpc",
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
    {
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        for command in commands {
            serde_json::to_writer(&mut stdin, command)?;
            stdin.write_all(b"\n")?;
        }
    }
    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut frames = Vec::new();
    for line in BufReader::new(stdout).lines() {
        frames.push(serde_json::from_str(&line?)?);
    }
    child.wait()?;
    Ok(frames)
}

fn responses(frames: &[Value]) -> Vec<&Value> {
    frames
        .iter()
        .filter(|frame| frame["type"] == "response")
        .collect()
}

#[test]
fn responds_per_command_streams_events_and_persists_v4() -> TestResult {
    let dir = temp_dir("turn")?;
    let frames = run_rpc(
        &dir,
        &[
            serde_json::json!({"id": "s", "type": "get_state"}),
            serde_json::json!({"id": "p", "type": "prompt", "message": "hello"}),
            serde_json::json!({"id": "n", "type": "set_session_name", "name": "protocol-test"}),
        ],
    )?;

    let responses = responses(&frames);
    assert_eq!(responses.len(), 3);
    let state = responses
        .iter()
        .find(|frame| frame["id"] == "s")
        .ok_or("missing get_state response")?;
    assert_eq!(state["command"], "get_state");
    assert_eq!(state["success"], true);
    assert_eq!(state["data"]["model"]["provider"], "faux");
    assert_eq!(state["data"]["isStreaming"], false);
    assert_eq!(state["data"]["messageCount"], 0);

    let event_types: Vec<&str> = frames
        .iter()
        .filter(|frame| frame["type"] != "response")
        .filter_map(|frame| frame["type"].as_str())
        .collect();
    assert!(event_types.contains(&"agent_start"));
    assert!(event_types.contains(&"message_end"));
    assert_eq!(event_types.last(), Some(&"agent_end"));
    let message_ends = event_types
        .iter()
        .filter(|kind| **kind == "message_end")
        .count();
    assert!(message_ends >= 2, "user + assistant message_end expected");

    // The corpus also holds `family/` and `kernels/` (#580); the transcript's directory is
    // the one holding a `.jsonl`.
    let session_file = std::fs::read_dir(dir.join("sessions"))?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| std::fs::read_dir(entry.path()).ok())
        .flat_map(|files| files.filter_map(Result::ok))
        .find(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .ok_or("no session file")?;
    let content = std::fs::read_to_string(session_file.path())?;
    let lines: Vec<Value> = content
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    assert_eq!(lines[0]["kind"], "header");
    assert_eq!(lines[0]["version"], 4);
    let entry_roles: Vec<&str> = lines
        .iter()
        .filter(|line| line["kind"] == "entry")
        .filter_map(|line| line["message"]["role"].as_str())
        .collect();
    assert_eq!(entry_roles, vec!["user", "assistant"]);
    // An rpc prompt crossed the process boundary: host-minted, it replayed as a host notice.
    let typed = lines
        .iter()
        .find(|line| line["message"]["role"] == "user")
        .ok_or("no user entry")?;
    assert_eq!(typed["message"]["attribution"], "user", "{typed}");
    assert!(
        lines
            .iter()
            .any(|line| line["kind"] == "fact" && line["name"] == "protocol-test")
    );

    Ok(())
}

#[test]
fn rejects_unknown_commands_and_answers_queries() -> TestResult {
    let dir = temp_dir("queries")?;
    let frames = run_rpc(
        &dir,
        &[
            serde_json::json!({"id": "m", "type": "get_available_models"}),
            serde_json::json!({"id": "t", "type": "get_available_thinking_levels"}),
            serde_json::json!({"id": "x", "type": "export_html"}),
            serde_json::json!({"id": "u", "type": "no_such_command"}),
        ],
    )?;

    let responses = responses(&frames);
    let by_id = |id: &str| {
        responses
            .iter()
            .find(|frame| frame["id"] == id)
            .copied()
            .ok_or_else(|| format!("missing response {id}"))
    };
    let models = by_id("m")?;
    assert_eq!(models["success"], true);
    assert!(
        models["data"]["models"]
            .as_array()
            .is_some_and(|models| !models.is_empty())
    );
    let levels = by_id("t")?;
    assert_eq!(levels["data"]["levels"][0], "off");
    let unsupported = by_id("x")?;
    assert_eq!(unsupported["success"], false);
    assert!(
        unsupported["error"]
            .as_str()
            .is_some_and(|error| error.contains("unsupported"))
    );
    let unknown = by_id("u")?;
    assert_eq!(unknown["success"], false);

    Ok(())
}

#[test]
fn the_level_list_follows_the_model_and_a_set_reports_the_clamp() -> TestResult {
    let dir = temp_dir("levels")?;
    let frames = run_rpc(
        &dir,
        &[
            serde_json::json!({"id": "before", "type": "get_available_thinking_levels"}),
            serde_json::json!({
                "id": "switch", "type": "set_model",
                "provider": "anthropic", "modelId": "claude-fable-5"
            }),
            serde_json::json!({"id": "after", "type": "get_available_thinking_levels"}),
            serde_json::json!({"id": "set", "type": "set_thinking_level", "level": "max"}),
            serde_json::json!({"id": "state", "type": "get_state"}),
            serde_json::json!({"id": "bad", "type": "set_thinking_level", "level": "extreme"}),
        ],
    )?;
    let responses = responses(&frames);
    let by_id = |id: &str| {
        responses
            .iter()
            .find(|frame| frame["id"] == id)
            .copied()
            .ok_or_else(|| format!("missing response {id}"))
    };

    assert_eq!(
        by_id("before")?["data"]["levels"],
        serde_json::json!(["off"])
    );
    assert_eq!(by_id("switch")?["success"], true);
    let after = by_id("after")?["data"]["levels"].clone();
    assert_eq!(
        after,
        serde_json::json!(["minimal", "low", "medium", "high", "xhigh", "max"]),
        "fable cannot disable thinking and advertises both advanced tiers"
    );
    assert_eq!(by_id("set")?["data"]["level"], "max");
    assert_eq!(by_id("state")?["data"]["thinkingLevel"], "max");
    let bad = by_id("bad")?;
    assert_eq!(bad["success"], false);
    assert!(
        bad["error"]
            .as_str()
            .is_some_and(|error| error.contains("extreme")),
        "the rejection names the value: {bad}"
    );

    Ok(())
}

#[test]
fn heartbeat_and_advisor_surfaces_respond() -> TestResult {
    let dir = temp_dir("surfaces")?;
    let frames = run_rpc(
        &dir,
        &[
            serde_json::json!({"id": "h", "type": "heartbeat", "command": "/heartbeat every 10m run the tests"}),
            serde_json::json!({"id": "s", "type": "heartbeat", "command": "/heartbeat status"}),
            serde_json::json!({"id": "a", "type": "advisor_stats"}),
        ],
    )?;
    let responses = responses(&frames);
    let set = responses
        .iter()
        .find(|frame| frame["id"] == "h")
        .ok_or("missing heartbeat response")?;
    assert_eq!(set["success"], true);
    assert!(
        set["data"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("Heartbeat set")),
        "set must confirm: {set}"
    );
    let status = responses
        .iter()
        .find(|frame| frame["id"] == "s")
        .ok_or("missing status response")?;
    assert!(
        status["data"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("run the tests")),
        "status must list the job: {status}"
    );
    let stats = responses
        .iter()
        .find(|frame| frame["id"] == "a")
        .ok_or("missing advisor_stats response")?;
    assert!(
        stats["data"]["text"]
            .as_str()
            .is_some_and(|text| text.starts_with("advisor:")),
        "/advisor stats must render: {stats}"
    );
    Ok(())
}

#[test]
fn goal_surface_round_trips() -> TestResult {
    let dir = temp_dir("goal")?;
    let frames = run_rpc(
        &dir,
        &[
            serde_json::json!({"id": "c", "type": "goal", "action": "create", "objective": "ship it", "tokenBudget": 500}),
            serde_json::json!({"id": "g", "type": "goal", "action": "get"}),
            serde_json::json!({"id": "u", "type": "goal", "action": "update", "status": "complete"}),
            serde_json::json!({"id": "x", "type": "goal", "action": "update", "status": "paused"}),
        ],
    )?;
    let responses = responses(&frames);
    let create = responses
        .iter()
        .find(|frame| frame["id"] == "c")
        .ok_or("missing create response")?;
    assert_eq!(create["success"], true);
    assert_eq!(create["data"]["goal"]["objective"], "ship it");
    assert_eq!(create["data"]["goal"]["remainingTokens"], 500);
    let get = responses
        .iter()
        .find(|frame| frame["id"] == "g")
        .ok_or("missing get response")?;
    assert_eq!(get["data"]["goal"]["status"], "active");
    let update = responses
        .iter()
        .find(|frame| frame["id"] == "u")
        .ok_or("missing update response")?;
    assert_eq!(update["data"]["goal"]["status"], "complete");
    let rejected = responses
        .iter()
        .find(|frame| frame["id"] == "x")
        .ok_or("missing rejected response")?;
    assert_eq!(
        rejected["success"], false,
        "the model-facing surface must reject host-owned statuses (G2)"
    );
    Ok(())
}

/// The rpc peer is anything on the box (plan section 3.7), so a submit carries no authority: a
/// claimed actor is refused as an argument, and with no prompt in this process an administrative
/// op is refused outright, while the owner's own ops apply. Nothing by a user reaches the journal.
#[test]
fn a_socket_client_cannot_confirm_an_administrative_op() -> TestResult {
    let dir = temp_dir("plan-authority")?;
    let frames = run_rpc(
        &dir,
        &[
            serde_json::json!({"id": "i", "type": "plan", "action": "submit",
                "args": {"op": "init", "goal": "ship the seam", "todos": [{"label": "cut"}]}}),
            serde_json::json!({"id": "f", "type": "plan", "action": "submit",
                "args": {"op": "fuse_reset"}, "actor": "user://1", "confirmed": true}),
            serde_json::json!({"id": "a", "type": "plan", "action": "submit",
                "args": {"op": "fuse_reset", "actor": "user://1"}}),
            serde_json::json!({"id": "p", "type": "plan", "action": "submit",
                "args": {"op": "fuse_reset"}, "request_id": "peer-1"}),
            serde_json::json!({"id": "v", "type": "plan", "action": "submit",
                "args": {"op": "view"}}),
        ],
    )?;
    let responses = responses(&frames);
    let reply = |id: &str| -> Result<&Value, Box<dyn Error>> {
        responses
            .iter()
            .copied()
            .find(|frame| frame["id"] == id)
            .ok_or_else(|| format!("no response {id} in {frames:?}").into())
    };
    assert_eq!(reply("i")?["success"], true, "{}", reply("i")?);
    for claimed in ["f", "a"] {
        let refused = reply(claimed)?;
        assert_eq!(refused["success"], false, "{refused}");
        assert!(
            refused["error"]
                .as_str()
                .is_some_and(|error| error.contains("actor is not an argument")),
            "{refused}"
        );
    }
    let unconfirmed = reply("p")?;
    assert_eq!(unconfirmed["success"], false, "{unconfirmed}");
    assert!(
        unconfirmed["error"]
            .as_str()
            .is_some_and(|error| error.contains("confirmation")),
        "{unconfirmed}"
    );
    assert_eq!(reply("v")?["success"], true, "{}", reply("v")?);
    assert!(
        reply("v")?["data"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("cut")),
        "{}",
        reply("v")?
    );

    let mut recorded = String::new();
    for entry in std::fs::read_dir(dir.join(".yi/plans"))? {
        let path = entry?.path().join("ops.jsonl");
        if path.is_file() {
            recorded.push_str(&std::fs::read_to_string(path)?);
        }
    }
    assert!(recorded.contains(r#""op":"init""#), "{recorded}");
    assert!(
        !recorded.contains(r#""op":"fuse_reset""#) && !recorded.contains("user://"),
        "a peer reached the journal as a user: {recorded}"
    );
    Ok(())
}
