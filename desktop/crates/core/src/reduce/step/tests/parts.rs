//! Child agents, todos, the goal, cost and the bell's list. No recording holds these, so the
//! frames are Yi's own types serialised into the envelope the recordings show.

use serde_json::{Value, json};
use yi_types::event::{AgentEvent, AssistantMessageEvent, ToolResult};
use yi_types::goal::{Goal, GoalStatus};
use yi_types::message::{Content, join_text};
use yi_types::plan::doc::Todo;
use yi_types::subagent::{ChildActivity, ChildFlag, ChildId, ChildStatus, ChildUpdate};
use yi_types::todo::{Claim, PhaseName, TodoList, TodoPhase};

use super::{Check, address, feed, head, played, recording, rows, run, said, thread, update};
use crate::ids::{AgentId, Millis, ThreadId};
use crate::kind::Kind;
use crate::model::Model;
use crate::reduce::step::commands::{OPEN, SEEN};
use crate::reduce::threads::{Status, Want};
use crate::reduce::todos::Plan;
use crate::reduce::turns::{CallId, ToolState};
use crate::wire::frame::MAX_FRAME;
use crate::wire::replay::play_line;

const THREAD: &str = "01a1de97-bf6c-7514-9fae-b57db1eaa7e1";

fn child(tools: u64, flag: Option<ChildFlag>) -> ChildUpdate {
    ChildUpdate {
        id: ChildId("a1".to_owned()),
        name: "Tests · upload".to_owned(),
        status: ChildStatus::Running,
        activity: ChildActivity::Executing,
        tool_use_count: tools,
        token_count: 8400,
        cost: serde_json::Number::from_f64(0.03),
        answer_preview: None,
        error: None,
        exit: None,
        flag,
        session: None,
        role: Some("worker".to_owned()),
        task: Some("Write tests for the new limiter and run them.".to_owned()),
        elapsed_ms: Some(360_000),
    }
}

fn child_frame(child: &ChildUpdate) -> Result<Value, Box<dyn std::error::Error>> {
    let mut fields = serde_json::to_value(child)?;
    fields["sessionUpdate"] = Value::from("_yi/subagent_update");
    Ok(update(THREAD, fields))
}

#[test]
fn a_child_agent_is_one_row_that_its_updates_keep_current() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    feed(&mut model, 10, &child_frame(&child(1, None))?);
    feed(&mut model, 20, &child_frame(&child(3, None))?);
    let agents = &thread(&model, THREAD)?.agents;
    let [agent] = agents.as_slice() else {
        return Err(format!("{} agents for one child", agents.len()).into());
    };
    assert_eq!(agent.id, AgentId::new("a1"));
    assert_eq!(agent.update, child(3, None));
    assert_eq!(
        (agent.first_seen, agent.last_seen),
        (Millis(10), Millis(20))
    );
    Ok(())
}

#[test]
fn a_childs_own_events_fold_into_its_transcript_not_the_threads() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    let before = said(&thread(&model, THREAD)?.transcript);
    feed(&mut model, 1, &child_frame(&child(0, None))?);
    // The recorded answer's events again, as the server forwards a child's: with its id,
    // on the thread's own run of numbers. Rows 16 to 21 are events 4 to 8.
    let mut seq = 11;
    for (_, mut frame) in rows("turn.jsonl")?.into_iter().take(22).skip(16) {
        let fields = &mut frame["params"]["update"];
        if fields["sessionUpdate"] != "_yi/event" {
            continue;
        }
        fields["childId"] = Value::from("a1");
        fields["seq"] = Value::from(seq);
        seq += 1;
        feed(&mut model, 2, &frame);
    }
    let thread = thread(&model, THREAD)?;
    assert_eq!(
        said(&thread.transcript),
        before,
        "a child spoke in the thread"
    );
    let agent = thread.agents.first().ok_or("no agent")?;
    assert_eq!(said(&agent.transcript), ["assistant: faux: "]);
    Ok(())
}

#[test]
fn a_child_that_waits_on_the_user_is_on_the_bells_list() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    run(&mut model, SEEN, address(Kind::Thread, THREAD)?, None);
    assert_eq!(model.wire.attention().alerts, []);
    let note = "asks q1: profile the release build?".to_owned();
    let flag = ChildFlag::NeedsYou { note: note.clone() };
    feed(&mut model, 30, &child_frame(&child(1, Some(flag)))?);
    let alerts = model.wire.attention().alerts;
    let [alert] = alerts.as_slice() else {
        return Err(format!("{} alerts", alerts.len()).into());
    };
    assert_eq!(alert.want, Want::Child(AgentId::new("a1")));
    assert_eq!(
        (alert.since, alert.detail.as_str()),
        (Millis(30), note.as_str())
    );
    assert_eq!(model.wire.status(&ThreadId::new(THREAD)), Status::NeedsYou);
    Ok(())
}

#[test]
fn a_finished_thread_nobody_looked_at_is_done_until_it_is_seen() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    let id = ThreadId::new(THREAD);
    assert_eq!(model.wire.status(&id), Status::Done);
    let alerts = model.wire.attention().alerts;
    let wants: Vec<(&Want, &str)> = alerts
        .iter()
        .map(|a| (&a.want, a.detail.as_str()))
        .collect();
    assert_eq!(
        wants,
        [(&Want::Done, "faux: ")],
        "its last words are the detail"
    );
    run(&mut model, SEEN, address(Kind::Thread, THREAD)?, None);
    assert_eq!(model.wire.status(&id), Status::Idle);
    assert_eq!(model.wire.attention().alerts, []);
    Ok(())
}

#[test]
fn a_thread_a_view_shows_never_counts_as_unseen() -> Check {
    // Rows 0 to 8: the thread exists and its run has begun, but no state has changed.
    let (mut model, _) = played(&head("turn.jsonl", 9)?)?;
    run(&mut model, OPEN, address(Kind::Thread, THREAD)?, None);
    for (_, frame) in rows("turn.jsonl")?.into_iter().skip(9) {
        feed(&mut model, 1, &frame);
    }
    assert_eq!(thread(&model, THREAD)?.unseen(), 0);
    assert_eq!(model.wire.status(&ThreadId::new(THREAD)), Status::Idle);
    Ok(())
}

#[test]
fn a_running_thread_is_working_and_an_errored_one_failed() -> Check {
    let (mut model, _) = played(&head("turn.jsonl", 19)?)?;
    let id = ThreadId::new(THREAD);
    assert_eq!(model.wire.status(&id), Status::Working);
    assert_eq!(model.wire.attention().working, std::slice::from_ref(&id));
    // Yi's only word for a failed run: idle with this stop reason.
    let failed = json!({"sessionUpdate": "state_update", "state": "idle",
        "stopReason": "_yi/error"});
    feed(&mut model, 40, &update(THREAD, failed));
    assert_eq!(model.wire.status(&id), Status::Failed);
    let alerts = model.wire.attention().alerts;
    let wants: Vec<&Want> = alerts.iter().map(|alert| &alert.want).collect();
    assert_eq!(wants, [&Want::Failed]);
    assert_eq!(model.wire.attention().working, []);
    Ok(())
}

#[test]
fn todos_the_plan_and_the_goal_are_what_the_server_last_said() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    let list = TodoList {
        phases: vec![TodoPhase {
            name: PhaseName::new("Limit uploads")?,
            items: vec![Todo::from_text("Write the limiter")?],
            extra: serde_json::Map::new(),
        }],
        next_id: 2,
        extra: serde_json::Map::new(),
    };
    let todo = json!({"sessionUpdate": "_yi/todo", "list": serde_json::to_value(&list)?});
    feed(&mut model, 1, &update(THREAD, todo));
    let plan = json!({"sessionUpdate": "_yi/plan_progress",
        "plan": {"done": 1, "total": 3, "running": "Write the limiter"}});
    feed(&mut model, 2, &update(THREAD, plan));
    let goal = Goal {
        objective: "Uploads cannot be hammered".to_owned(),
        status: GoalStatus::Active,
        token_budget: Some(200_000),
        tokens_used: 41_200,
        time_used_seconds: 840,
        created: 1,
        updated: 2,
        check: Some("cargo test -p api upload::".to_owned()),
        check_timeout_ms: None,
        check_failure: None,
        discoveries: Vec::new(),
        extra: serde_json::Map::new(),
    };
    let frame = json!({"sessionUpdate": "_yi/goal", "goal": serde_json::to_value(&goal)?});
    feed(&mut model, 3, &update(THREAD, frame));

    let work = &thread(&model, THREAD)?.work;
    assert_eq!(work.todos.as_deref(), Some(&list));
    assert_eq!(
        work.plan,
        Some(Plan {
            done: 1,
            total: 3,
            running: Some("Write the limiter".to_owned())
        })
    );
    assert_eq!(work.goal.as_deref(), Some(&goal));

    let cleared = json!({"sessionUpdate": "_yi/todo", "list": null});
    feed(&mut model, 4, &update(THREAD, cleared));
    assert_eq!(thread(&model, THREAD)?.work.todos, None);
    Ok(())
}

#[test]
fn a_threads_title_and_its_claims_are_what_the_server_last_said() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    let claim = Claim {
        label: "Write the limiter".to_owned(),
        observed: Some("c1".to_owned()),
    };
    let claims = json!({"sessionUpdate": "_yi/claims", "claims": [serde_json::to_value(&claim)?]});
    feed(&mut model, 1, &update(THREAD, claims));
    let name = json!({"sessionUpdate": "_yi/name", "name": "Rate-limit the upload endpoint"});
    feed(&mut model, 2, &update(THREAD, name));
    let thread = thread(&model, THREAD)?;
    assert_eq!(thread.work.claims, [claim]);
    assert_eq!(
        thread.title.as_deref(),
        Some("Rate-limit the upload endpoint")
    );
    Ok(())
}

#[test]
fn cost_is_the_servers_ledger_plus_the_replies_since_never_both() -> Check {
    let mut model = Model::default();
    for (index, line) in recording("resume.jsonl")?.lines().enumerate() {
        // Rows 50 and 59: the last answer's end, and the full replay with the ledger.
        let line = if index == 50 || index == 59 {
            priced(line)?
        } else {
            line.to_owned()
        };
        play_line(&mut model, &line)?;
        if index == 50 || index == 60 {
            let thread = thread(&model, "01a1de97-c28d-7c67-a51d-8d977b7c7930")?;
            assert_eq!(thread.cost(), 0.25, "row {index}: a reply is counted once");
        }
    }
    Ok(())
}

/// A recorded line with a price put on it: the faux model's replies all cost nothing.
fn priced(line: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut row: Value = serde_json::from_str(line)?;
    let fields = &mut row["frame"]["params"]["update"];
    if fields["sessionUpdate"] == "_yi/replay" {
        fields["stats"]["costTotal"] = json!(0.25);
    } else {
        fields["event"]["message"]["usage"]["cost"]["total"] = json!(0.25);
    }
    Ok(row.to_string())
}

#[test]
fn a_stream_cannot_grow_a_message_past_one_frames_size() -> Check {
    // Row 17 opens the answer's text block; its end would have to fit one frame.
    let (mut model, _) = played(&head("turn.jsonl", 18)?)?;
    let megabyte = "x".repeat(1 << 20);
    for seq in 6..46 {
        let event = json!({"type": "message_update", "assistantMessageEvent":
            {"type": "text_delta", "contentIndex": 0, "delta": megabyte}});
        let frame = json!({"sessionUpdate": "_yi/event", "event": event, "seq": seq});
        feed(&mut model, 1, &update(THREAD, frame));
    }
    let said = said(&thread(&model, THREAD)?.transcript);
    let streamed = said.last().map_or(0, String::len);
    let cap = usize::try_from(MAX_FRAME)?;
    assert!(
        streamed <= cap + 32,
        "{streamed} bytes streamed into one message"
    );
    assert!(
        streamed >= cap - (1 << 20),
        "the cap cut far too early: {streamed}"
    );
    Ok(())
}

#[test]
fn an_ask_for_a_childs_tool_call_marks_the_childs_call_and_names_the_child() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    feed(&mut model, 1, &child_frame(&child(0, None))?);
    let event =
        |seq: u64, child: Option<&str>, event: AgentEvent| -> Result<Value, serde_json::Error> {
            let mut fields = json!({"sessionUpdate": "_yi/event",
            "event": serde_json::to_value(event)?, "seq": seq});
            if let Some(child) = child {
                fields["childId"] = Value::from(child);
            }
            Ok(update(THREAD, fields))
        };
    let started = AgentEvent::ToolExecutionStart {
        tool_call_id: "k1".to_owned(),
        tool_name: "bash".to_owned(),
        args: json!({"command": "cargo test"}),
    };
    feed(&mut model, 2, &event(11, Some("a1"), started)?);
    // The server sends a child's ask on the thread's own stream, naming no child.
    let asked = AgentEvent::PermissionRequested {
        tool_call_id: "k1".to_owned(),
        title: "bash requires permission".to_owned(),
        description: "cargo test".to_owned(),
    };
    feed(&mut model, 3, &event(12, None, asked)?);
    let thread = thread(&model, THREAD)?;
    assert!(
        !thread.transcript.tools.contains_key("k1"),
        "the thread got a call it never made"
    );
    let asker = thread
        .asker(&CallId::new("k1"))
        .ok_or("nobody is known to ask")?;
    assert_eq!(asker.id, AgentId::new("a1"));
    let call = asker.transcript.tools.get("k1").ok_or("no call k1")?;
    assert_eq!(call.state, ToolState::Asking);
    Ok(())
}

/// One agent event in the envelope the recordings show, numbered `seq`.
fn event(seq: u64, event: &AgentEvent) -> Result<Value, serde_json::Error> {
    let fields = json!({"sessionUpdate": "_yi/event",
        "event": serde_json::to_value(event)?, "seq": seq});
    Ok(update(THREAD, fields))
}

#[test]
fn thinking_streams_into_its_own_block_before_the_answer() -> Check {
    // Row 16 starts the recorded answer; the faux model never thinks, so these events
    // are Yi's own types for a model that does.
    let (mut model, _) = played(&head("turn.jsonl", 17)?)?;
    let thinking = [
        AssistantMessageEvent::ThinkingStart { content_index: 0 },
        AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: "the route is ".to_owned(),
        },
        AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: "in upload.rs".to_owned(),
        },
        AssistantMessageEvent::TextStart { content_index: 1 },
        AssistantMessageEvent::TextDelta {
            content_index: 1,
            delta: "Found it.".to_owned(),
        },
    ];
    for (seq, assistant_message_event) in (5..).zip(thinking) {
        let update = AgentEvent::MessageUpdate {
            assistant_message_event,
        };
        feed(&mut model, 1, &event(seq, &update)?);
    }
    assert_eq!(
        said(&thread(&model, THREAD)?.transcript)
            .last()
            .map(String::as_str),
        Some("assistant: (the route is in upload.rs) Found it.")
    );
    Ok(())
}

#[test]
fn a_tools_output_shows_while_it_still_runs() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    let running = AgentEvent::ToolExecutionUpdate {
        tool_call_id: "c7".to_owned(),
        tool_name: "bash".to_owned(),
        args: json!({"command": "cargo test"}),
        partial_result: ToolResult {
            content: vec![Content::Text {
                text: "running 12 tests".to_owned(),
                text_signature: None,
            }],
            details: Value::Null,
            usage: None,
            added_tool_names: None,
            terminate: None,
        },
    };
    feed(&mut model, 50, &event(11, &running)?);
    let call = thread(&model, THREAD)?.transcript.tools.get("c7");
    let call = call.ok_or("the running call is not in the model")?;
    assert_eq!(call.state, ToolState::Running);
    assert_eq!(join_text(&call.output, ""), "running 12 tests");
    assert_eq!((call.started, call.ended), (Some(Millis(50)), None));
    Ok(())
}

fn compaction(id: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let entry = yi_types::entry::Entry::Compaction {
        id: id.to_owned(),
        summary: "The limiter is in; tests next.".to_owned(),
        retained_tail: Vec::new(),
        tokens_before: 151_000,
        details: None,
        usage: None,
        parent_id: None,
        seq: 9,
        timestamp: 1_794_679_100_000,
    };
    let entry = serde_json::to_value(entry)?;
    Ok(update(
        THREAD,
        json!({"sessionUpdate": "_yi/committed", "entry": entry}),
    ))
}

#[test]
fn a_stored_compaction_counts_once_however_often_it_is_sent() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    assert_eq!(thread(&model, THREAD)?.transcript.compactions, 0);
    feed(&mut model, 10, &compaction("c-1")?);
    feed(&mut model, 20, &compaction("c-1")?);
    assert_eq!(
        thread(&model, THREAD)?.transcript.compactions,
        1,
        "sent twice"
    );
    feed(&mut model, 30, &compaction("c-2")?);
    assert_eq!(thread(&model, THREAD)?.transcript.compactions, 2);
    Ok(())
}
