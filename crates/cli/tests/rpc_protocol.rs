use std::error::Error;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use serde_json::{Value, json};

use crate::rpc::{run_rpc, spawn_rpc};
use crate::scratch::Scratch;

use crate::prompt_slots_support::prompt_slots;
use crate::session_files::session_files;

type TestResult = Result<(), Box<dyn Error>>;

fn temp_dir(tag: &str) -> Result<Scratch, Box<dyn Error>> {
    Ok(Scratch::new(&format!("yi-rpc-{tag}"))?)
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

    let session_file = session_files(&dir.join("sessions"))?
        .into_iter()
        .next()
        .ok_or("no session file")?;
    let content = std::fs::read_to_string(session_file)?;
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

/// Dies with a person's session told nobody is there: an rpc client wires no asker, as `yi ask`
/// with no terminal does, yet its session records no unattended slot.
#[test]
fn an_rpc_session_is_not_told_it_is_alone() -> TestResult {
    let dir = temp_dir("attended")?;
    run_rpc(
        &dir,
        &[serde_json::json!({"id": "p", "type": "prompt", "message": "hello"})],
    )?;
    let names: Vec<String> = prompt_slots(&dir.join("sessions"))?.concat();
    assert!(
        names.iter().any(|name| name == "doctrine"),
        "no prompt slots recorded: {names:?}"
    );
    assert!(!names.iter().any(|name| name == "unattended"), "{names:?}");
    Ok(())
}

/// Faux replies: `(ask_chars, reply_chars, lag_ms, priced)` per request, the lag being how long
/// the reply takes to land, so a command that awaits it is measurably in flight.
fn cassette(dir: &Path, replies: &[(usize, usize, u64, bool)]) -> Result<PathBuf, Box<dyn Error>> {
    let usage = |priced: bool| {
        let tokens = if priced { 200_000 } else { 0 };
        json!({"input": tokens, "output": 10, "cacheRead": 0, "cacheWrite": 0,
               "totalTokens": tokens, "cost": {"input": 0, "output": 0, "cacheRead": 0,
                                               "cacheWrite": 0, "total": 0}})
    };
    let mut lines: Vec<String> = Vec::new();
    let mut at = 0_u64;
    for (ask_chars, reply_chars, lag_ms, priced) in replies {
        at = at.saturating_add(100);
        lines.push(
            json!({"kind": "entry", "lane": "main", "type": "message", "id": format!("u{at}"),
                   "parentId": null, "seq": 1, "timestamp": at,
                   "message": {"role": "user", "content": "q".repeat(*ask_chars),
                               "timestamp": at}})
            .to_string(),
        );
        let landed = at.saturating_add(*lag_ms);
        lines.push(
            json!({"kind": "entry", "lane": "main", "type": "message", "id": format!("a{landed}"),
                   "parentId": null, "seq": 1, "timestamp": landed,
                   "message": {"role": "assistant", "api": "faux", "provider": "faux",
                               "model": "faux-1",
                               "content": [{"type": "text",
                                            "text": "word ".repeat(reply_chars / 5)}],
                               "usage": usage(*priced), "stopReason": "stop",
                               "timestamp": landed}})
            .to_string(),
        );
    }
    let path = dir.join("cassette.jsonl");
    std::fs::write(&path, lines.join("\n"))?;
    Ok(path)
}

/// An rpc child whose stdin stays open, so a command can arrive while another is still in flight.
struct Live {
    child: Child,
    stdin: ChildStdin,
    frames: Receiver<Value>,
}

impl Live {
    fn start(dir: &Path, cassette: &Path) -> Result<Self, Box<dyn Error>> {
        let (child, stdin, frames) = spawn_rpc(dir, Some(cassette))?;
        Ok(Self {
            child,
            stdin,
            frames,
        })
    }

    fn send(&mut self, command: Value) -> Result<(), Box<dyn Error>> {
        serde_json::to_writer(&mut self.stdin, &command)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn next_frame(&mut self) -> Result<Option<Value>, Box<dyn Error>> {
        self.next_frame_within(Duration::from_secs(120))
    }

    fn next_frame_within(&mut self, limit: Duration) -> Result<Option<Value>, Box<dyn Error>> {
        match self.frames.recv_timeout(limit) {
            Ok(frame) => Ok(Some(frame)),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => Ok(None),
        }
    }

    fn until(&mut self, wanted: impl Fn(&Value) -> bool) -> Result<Option<Value>, Box<dyn Error>> {
        while let Some(frame) = self.next_frame()? {
            if wanted(&frame) {
                return Ok(Some(frame));
            }
        }
        Ok(None)
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A command the loop answers inline parks the event arm, and a turn streaming into the broadcast
/// buffer overflows it: the client loses frames it was never told it lost.
#[test]
fn a_command_in_flight_does_not_starve_the_event_stream() -> TestResult {
    let dir = temp_dir("busy")?;
    let stream = cassette(&dir, &[(5, 100_000, 0, false)])?;
    let mut live = Live::start(&dir, &stream)?;
    live.send(json!({"id": "p", "type": "prompt", "message": "hello"}))?;
    let (mut asked, mut answered, mut gaps) = (0_u64, 0_u64, 0_u64);
    let mut ended = false;
    loop {
        let quiet = Duration::from_secs(1);
        let next = if ended {
            live.next_frame_within(quiet)?
        } else {
            live.next_frame()?
        };
        let Some(frame) = next else { break };
        match frame["type"].as_str().unwrap_or_default() {
            "event_gap" => gaps = gaps.saturating_add(1),
            "message_update" if asked < 160 => {
                for _ in 0..4 {
                    live.send(json!({
                        "id": format!("g{asked}"),
                        "type": "goal",
                        "action": "get",
                    }))?;
                    asked = asked.saturating_add(1);
                }
            }
            "response" if frame["command"] == "goal" => {
                answered = answered.saturating_add(1);
            }
            "agent_end" => ended = true,
            _ => {}
        }
        if ended && asked > 0 && answered == asked {
            break;
        }
    }
    assert!(
        asked > 0,
        "the turn streamed no update to answer a command on"
    );
    assert_eq!(asked, answered, "every command was answered");
    assert_eq!(
        gaps, 0,
        "answering a command dropped {gaps} event frame(s) the client never asked to skip"
    );
    Ok(())
}

/// A compact queued behind a blocking goal check has not started: it must not let a state query
/// jump ahead of the session switch whose answer the query is meant to observe.
#[test]
fn a_queued_compact_does_not_reorder_a_state_query() -> TestResult {
    let dir = temp_dir("queued-compact")?;
    let stream = cassette(&dir, &[])?;
    let mut live = Live::start(&dir, &stream)?;
    live.send(json!({"id": "before", "type": "get_state"}))?;
    let initial = live
        .until(|frame| frame["type"] == "response" && frame["id"] == "before")?
        .ok_or("the initial state never answered")?;
    let old_id = initial["data"]["sessionId"]
        .as_str()
        .ok_or("the initial state has no session id")?
        .to_owned();
    live.send(json!({"id": "g", "type": "goal", "action": "create",
                     "objective": "hold the queue", "check": "sleep 2",
                     "checkTimeoutMs": 5_000}))?;
    assert!(
        live.until(|frame| frame["type"] == "response" && frame["id"] == "g")?
            .is_some_and(|frame| frame["success"] == true),
        "the checked goal was not created"
    );
    live.send(json!({"id": "u", "type": "goal", "action": "update", "status": "complete"}))?;
    live.send(json!({"id": "c", "type": "compact"}))?;
    live.send(json!({"id": "n", "type": "new_session"}))?;
    live.send(json!({"id": "s", "type": "get_state"}))?;
    let mut order = Vec::new();
    let mut new_id = None;
    while order.len() < 2 {
        let frame = live
            .next_frame()?
            .ok_or("the switch and state query did not both answer")?;
        if frame["type"] == "response" && (frame["id"] == "n" || frame["id"] == "s") {
            order.push(
                frame["id"]
                    .as_str()
                    .ok_or("response id is not text")?
                    .to_owned(),
            );
            if frame["id"] == "s" {
                new_id = frame["data"]["sessionId"].as_str().map(str::to_owned);
            }
        }
    }
    assert_eq!(order, ["n", "s"], "the state query jumped the switch");
    assert_ne!(new_id.as_deref(), Some(old_id.as_str()));
    Ok(())
}

/// A compact over a paced summary takes seconds; a client asking its state meanwhile must hear
/// about it, not a constant false delivered behind the compact's own reply.
#[test]
fn a_compact_in_flight_reports_compacting() -> TestResult {
    let dir = temp_dir("compacting")?;
    let paced = cassette(
        &dir,
        &[
            (100_000, 10, 0, true),
            (100_000, 15, 0, true),
            (0, 20, 1_200, false),
            (0, 20, 1_200, false),
        ],
    )?;
    let mut live = Live::start(&dir, &paced)?;
    for turn in 0..2 {
        live.send(json!({"id": format!("p{turn}"), "type": "prompt",
                         "message": "z".repeat(100_000)}))?;
        assert!(
            live.until(|frame| frame["type"] == "agent_end")?.is_some(),
            "turn {turn} never ended"
        );
    }
    live.send(json!({"id": "c", "type": "compact"}))?;
    // Dispatch the polls without waiting on the first: before the runtime raises its flag, that reply
    // is ordered behind the compact and would otherwise stop any later poll reaching the runtime.
    for attempt in 0..400_u32 {
        let id = format!("s{attempt}");
        live.send(json!({"id": id, "type": "get_state"}))?;
    }
    let mut in_flight = None;
    for _ in 0..400_u32 {
        let Some(state) = live.until(|frame| frame["command"] == "get_state")? else {
            break;
        };
        if state["data"]["isCompacting"] == true {
            in_flight = Some(state);
            break;
        }
    }
    assert!(
        in_flight.is_some(),
        "a compact was in flight and no state query said so"
    );
    assert!(
        live.until(|frame| frame["type"] == "response" && frame["id"] == "c")?
            .is_some(),
        "the compact never answered"
    );
    Ok(())
}

/// The rpc peer speaks Pi's command names; a surface this build cannot honour must say so with a
/// reason, and the state must not echo a mode nothing reads.
#[test]
fn pi_compatibility_surfaces_answer_what_this_build_honours() -> TestResult {
    let dir = temp_dir("pi-surfaces")?;
    let frames = run_rpc(
        &dir,
        &[
            json!({"id": "c", "type": "get_commands"}),
            json!({"id": "s", "type": "set_steering_mode", "mode": "all"}),
            json!({"id": "f", "type": "set_follow_up_mode", "mode": "all"}),
            json!({"id": "g", "type": "get_state"}),
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
    let commands = reply("c")?;
    assert_eq!(commands["success"], true, "{commands}");
    let verbs: Vec<&str> = commands["data"]["commands"]
        .as_array()
        .ok_or("no commands array")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        verbs,
        [
            "compact",
            "compact_status",
            "prompt",
            "heartbeat",
            "goal",
            "plan",
            "advisor_stats",
            "steer",
            "follow_up",
            "abort",
            "new_session",
            "get_state",
            "set_model",
            "get_available_models",
            "set_thinking_level",
            "get_available_thinking_levels",
            "set_steering_mode",
            "set_follow_up_mode",
            "get_messages",
            "get_entries",
            "get_session_stats",
            "set_session_name",
            "get_last_assistant_text",
            "get_commands",
            "switch_session",
            "fork",
            "get_log",
        ],
        "get_commands must name the RPC commands, not unrouted slash verbs: {commands}"
    );
    for id in ["s", "f"] {
        let refused = reply(id)?;
        assert_eq!(refused["success"], false, "{refused}");
        assert!(
            refused["error"]
                .as_str()
                .is_some_and(|error| error.contains("no mode to set")),
            "a refusal names why: {refused}"
        );
    }
    let state = reply("g")?;
    assert!(
        state["data"]["sessionId"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "the state names its session: {state}"
    );
    assert!(
        state["data"].get("steeringMode").is_none() && state["data"].get("followUpMode").is_none(),
        "the state echoes a mode nothing reads: {state}"
    );
    Ok(())
}
