use std::error::Error;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::Value;

type TestResult = Result<(), Box<dyn Error>>;

fn temp_dir(tag: &str) -> Result<std::path::PathBuf, Box<dyn Error>> {
    let dir = std::env::temp_dir().join(format!("yi-rpc-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
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

    let sessions_dir = std::fs::read_dir(dir.join("sessions"))?
        .next()
        .ok_or("no cwd session directory")??;
    let session_file = std::fs::read_dir(sessions_dir.path())?
        .filter_map(Result::ok)
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
    assert!(
        lines
            .iter()
            .any(|line| line["kind"] == "fact" && line["name"] == "protocol-test")
    );

    std::fs::remove_dir_all(&dir)?;
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

    std::fs::remove_dir_all(&dir)?;
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
    let _ = std::fs::remove_dir_all(&dir);
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
