//! The queue of permission asks: kept here, answered by the kept id.

use serde_json::{Value, json};
use yi_types::acp::{AcpPermissionOptionKind, AcpStopReason};
use yi_types::message::join_text;

use super::{Check, address, branch, feed, head, played, recording, rows, run, said, sent, thread};
use crate::command::table::Target;
use crate::ids::{ApprovalId, Millis, ThreadId};
use crate::kind::Kind;
use crate::model::Model;
use crate::msg::{Msg, step};
use crate::reduce::approvals::{MAX_ASKS, OptionKind};
use crate::reduce::step::commands::{
    ALLOW, ALWAYS, ANSWER, CANCEL, CANCEL_ASKS, CLOSE, DENY, OPEN,
};
use crate::reduce::threads::{Alert, Run, Status, Want};
use crate::reduce::turns::{CallId, ToolCall, ToolState};
use crate::wire::event::{Down, WireEvent};

const ALLOWED: &str = "01a1de97-c166-7d2e-9ca8-5dee496589e1";
const REJECTED: &str = "01a1de97-c223-744c-8afa-018c5c9ad664";
const ASK: &str = "perm_01a1de97-c166-7d2e-9ca8-5dee496589e1_1";
const WHY: &str = "tool invocation is not read-only: echo one > one.txt";

/// The model once the ask of `approval-allow.jsonl` has arrived: rows 0 to 28.
fn asked() -> Result<Model, Box<dyn std::error::Error>> {
    Ok(played(&head("approval-allow.jsonl", 29)?)?.0)
}

fn tool<'a>(model: &'a Model, thread_id: &str) -> Result<&'a ToolCall, Box<dyn std::error::Error>> {
    let tools = &thread(model, thread_id)?.transcript.tools;
    Ok(tools.get("c1").ok_or("the call c1 is not in the model")?)
}

#[test]
fn an_ask_waits_in_the_queue_beside_its_tool_call() -> Check {
    let model = asked()?;
    let [ask] = model.wire.asks.as_slice() else {
        return Err(format!("{} asks are queued", model.wire.asks.len()).into());
    };
    assert_eq!(ask.id, ApprovalId::new(ASK));
    assert_eq!(ask.thread, ThreadId::new(ALLOWED));
    assert_eq!(ask.title, "bash requires permission");
    assert_eq!(ask.description.as_deref(), Some(WHY));
    let kinds: Vec<&AcpPermissionOptionKind> = ask.options.iter().map(|o| &o.kind).collect();
    assert_eq!(
        kinds,
        [
            &AcpPermissionOptionKind::AllowOnce,
            &AcpPermissionOptionKind::AllowAlways,
            &AcpPermissionOptionKind::RejectOnce
        ]
    );
    assert_eq!(
        ask.tool,
        Some(CallId::new("c1")),
        "the event names its call"
    );
    assert_eq!(ask.answer, None);

    let call = tool(&model, ALLOWED)?;
    assert_eq!(
        (call.name.as_str(), call.state),
        ("bash", ToolState::Asking)
    );
    assert_eq!(call.args["command"], "echo one > one.txt");
    assert_eq!(thread(&model, ALLOWED)?.state, Run::NeedsYou);
    Ok(())
}

#[test]
fn a_waiting_ask_is_what_the_bell_shows_first() -> Check {
    let model = asked()?;
    let id = ThreadId::new(ALLOWED);
    assert_eq!(model.wire.status(&id), Status::NeedsYou);
    assert_eq!(
        model.wire.attention().alerts,
        [Alert {
            thread: id,
            want: Want::Approval(ApprovalId::new(ASK)),
            since: Millis(0),
            detail: WHY.to_owned()
        }]
    );
    Ok(())
}

#[test]
fn an_allowed_command_runs_and_its_output_lands_on_the_call() -> Check {
    let (model, _) = played(&recording("approval-allow.jsonl")?)?;
    assert!(model.wire.asks.is_empty(), "an answered ask left the queue");
    let call = tool(&model, ALLOWED)?;
    assert_eq!(call.state, ToolState::Done);
    assert_eq!(join_text(&call.output, "\n"), "(no output)");
    assert_eq!(call.details["exitCode"], 0);
    assert_eq!(call.duration(), Some(Millis(73)));
    assert_eq!(call.gate.and_then(|gate| gate.allowed), Some(true));
    let thread = thread(&model, ALLOWED)?;
    assert_eq!(
        said(&thread.transcript),
        [
            "user: run the command",
            "assistant: [bash c1]",
            "assistant: turn done"
        ]
    );
    let turns = thread.transcript.turns();
    assert_eq!(turns.len(), 1, "one prompt is one turn");
    assert_eq!(
        turns.first().map(|turn| (turn.len, turn.steps)),
        Some((3, 1))
    );
    assert_eq!(
        thread.state,
        Run::Idle {
            stop: Some(AcpStopReason::EndTurn)
        }
    );
    Ok(())
}

#[test]
fn a_denied_command_fails_with_the_servers_words() -> Check {
    let (model, _) = played(&recording("approval-reject.jsonl")?)?;
    assert!(model.wire.asks.is_empty());
    let call = tool(&model, REJECTED)?;
    assert_eq!(call.state, ToolState::Failed);
    assert_eq!(
        join_text(&call.output, "\n"),
        format!("Permission denied: The user denied this call. {WHY}")
    );
    assert_eq!(call.gate.and_then(|gate| gate.allowed), Some(false));
    assert_eq!(call.duration(), Some(Millis(2)));
    assert_eq!(
        said(&thread(&model, REJECTED)?.transcript)
            .last()
            .map(String::as_str),
        Some("assistant: turn done")
    );
    Ok(())
}

/// The frame the recording's client sent at row 29: the answer to the ask.
fn recorded_answer(name: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let (_, frame) = rows(name)?.into_iter().nth(29).ok_or("no row 29")?;
    Ok(frame)
}

#[test]
fn allow_and_deny_answer_under_the_kept_id_as_recorded() -> Check {
    let mut model = asked()?;
    let ask = address(Kind::Approval, ASK)?;
    let allowed = sent(&run(&mut model, ALLOW, ask.clone(), None))?;
    assert_eq!(allowed, [recorded_answer("approval-allow.jsonl")?]);
    let chosen = model
        .wire
        .asks
        .first()
        .and_then(|ask| ask.answer.as_deref());
    assert_eq!(
        chosen,
        Some("allow_once"),
        "the choice is kept until confirmed"
    );

    let mut model = asked()?;
    let denied = sent(&run(&mut model, DENY, ask.clone(), None))?;
    let mut recorded = recorded_answer("approval-reject.jsonl")?;
    recorded["id"] = Value::from(ASK);
    assert_eq!(denied, [recorded]);

    let mut model = asked()?;
    let always = sent(&run(&mut model, ALWAYS, ask, None))?;
    assert_eq!(
        always
            .first()
            .map(|frame| &frame["result"]["outcome"]["optionId"]),
        Some(&Value::from("allow_always"))
    );
    Ok(())
}

#[test]
fn a_chosen_answer_is_sent_again_after_a_reconnect() -> Check {
    let mut model = asked()?;
    let first = sent(&run(&mut model, ALLOW, address(Kind::Approval, ASK)?, None))?;
    let lost = Down::Closed("the server went away".to_owned());
    step(
        &mut model,
        Msg::Wire(WireEvent::Down {
            at: Millis(5),
            why: lost,
        }),
    );
    assert_eq!(
        model.wire.asks.len(),
        1,
        "a reconnect must not lose the ask"
    );
    let up = WireEvent::Up {
        at: Millis(9),
        server: "0.1041.0".to_owned(),
        latency: None,
    };
    let again = sent(&step(&mut model, Msg::Wire(up)))?;
    assert_eq!(again, first, "the server never sends an ask twice");
    Ok(())
}

#[test]
fn closing_the_last_view_answers_its_asks_cancelled() -> Check {
    let mut model = asked()?;
    let thread = address(Kind::Thread, ALLOWED)?;
    run(&mut model, OPEN, thread.clone(), None);
    run(&mut model, OPEN, thread.clone(), None);
    let first = sent(&run(&mut model, CLOSE, thread.clone(), None))?;
    assert_eq!(first, [Value::Null; 0], "another view still shows it");
    assert_eq!(model.wire.asks.len(), 1);
    let last = sent(&run(&mut model, CLOSE, thread, None))?;
    assert_eq!(
        last,
        [json!({"jsonrpc": "2.0", "id": ASK, "result": {"outcome": {"outcome": "cancelled"}}})]
    );
    assert!(model.wire.asks.is_empty());
    Ok(())
}

/// The recorded ask, row 26 of `approval-allow.jsonl`, under another id.
fn ask_frame(id: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let (_, mut frame) = rows("approval-allow.jsonl")?
        .into_iter()
        .nth(26)
        .ok_or("no row 26")?;
    frame["id"] = Value::from(id);
    Ok(frame)
}

#[test]
fn an_ask_this_app_cannot_read_is_answered_cancelled_at_once() -> Check {
    let mut model = Model::default();
    let mut frame = ask_frame("perm_x_1")?;
    // A newer server's ask with no options: there is nothing to offer the user.
    frame["params"]
        .as_object_mut()
        .ok_or("the ask has no params")?
        .remove("options");
    let answers = sent(&feed(&mut model, 1, &frame))?;
    assert_eq!(
        answers,
        [json!({"jsonrpc": "2.0", "id": "perm_x_1",
            "result": {"outcome": {"outcome": "cancelled"}}})],
        "an ask nobody can answer would block the run for good"
    );
    assert!(model.wire.asks.is_empty());
    Ok(())
}

#[test]
fn the_queue_holds_its_cap_and_refuses_the_next_ask() -> Check {
    let mut model = Model::default();
    for n in 0..MAX_ASKS {
        let answers = sent(&feed(&mut model, 1, &ask_frame(&format!("perm_x_{n}"))?))?;
        assert_eq!(answers, [Value::Null; 0], "ask {n} fits the queue");
    }
    assert_eq!(model.wire.asks.len(), MAX_ASKS);
    let over = sent(&feed(&mut model, 2, &ask_frame("perm_x_over")?))?;
    assert_eq!(
        over.first().map(|frame| (&frame["id"], &frame["result"])),
        Some((
            &Value::from("perm_x_over"),
            &json!({"outcome": {"outcome": "cancelled"}})
        ))
    );
    assert_eq!(model.wire.asks.len(), MAX_ASKS);
    Ok(())
}

#[test]
fn an_answered_ask_no_longer_asks_for_the_user() -> Check {
    let mut model = asked()?;
    run(&mut model, ALLOW, address(Kind::Approval, ASK)?, None);
    assert_eq!(model.wire.asks.len(), 1, "kept until the server confirms");
    assert_eq!(model.wire.waiting().count(), 0);
    assert_eq!(model.wire.attention().alerts, []);
    Ok(())
}

#[test]
fn an_ask_that_came_without_its_event_goes_when_its_answer_is_confirmed() -> Check {
    // An ask held back while no client watched comes alone: its event is long past.
    let mut model = Model::default();
    feed(&mut model, 1, &ask_frame("perm_x_1")?);
    let ask = address(Kind::Approval, "perm_x_1")?;
    run(&mut model, ALLOW, ask, None);
    assert_eq!(model.wire.asks.len(), 1);
    // Row 30 of the recording: the server says the call's ask was answered.
    let (_, resolved) = rows("approval-allow.jsonl")?
        .into_iter()
        .nth(30)
        .ok_or("no row 30")?;
    feed(&mut model, 2, &resolved);
    assert!(model.wire.asks.is_empty(), "a confirmed ask stayed queued");
    Ok(())
}

#[test]
fn closing_a_view_while_offline_keeps_its_asks_to_answer_later() -> Check {
    let mut model = asked()?;
    let thread = address(Kind::Thread, ALLOWED)?;
    run(&mut model, OPEN, thread.clone(), None);
    let lost = Down::Closed("the server went away".to_owned());
    step(
        &mut model,
        Msg::Wire(WireEvent::Down {
            at: Millis(5),
            why: lost,
        }),
    );
    let answers = sent(&run(&mut model, CLOSE, thread, None))?;
    assert_eq!(
        answers,
        [Value::Null; 0],
        "an answer now would reach nobody"
    );
    let quitting = sent(&run(&mut model, CANCEL_ASKS, Target::App, None))?;
    assert_eq!(quitting, [Value::Null; 0]);
    assert_eq!(model.wire.asks.len(), 1, "the server never asks twice");
    Ok(())
}

#[test]
fn deny_cancels_an_ask_that_offers_no_way_to_say_no() -> Check {
    let mut model = Model::default();
    let mut frame = ask_frame("perm_x_1")?;
    // The recorded ask with its reject option taken out.
    let options = frame["params"]["options"]
        .as_array_mut()
        .ok_or("the ask has no options")?;
    options.retain(|option| option["kind"] != "reject_once");
    feed(&mut model, 1, &frame);
    let denied = sent(&run(
        &mut model,
        DENY,
        address(Kind::Approval, "perm_x_1")?,
        None,
    ))?;
    assert_eq!(
        denied,
        [json!({"jsonrpc": "2.0", "id": "perm_x_1",
            "result": {"outcome": {"outcome": "cancelled"}}})]
    );
    assert!(model.wire.asks.is_empty());
    Ok(())
}

#[test]
fn a_command_that_names_no_ask_answers_the_oldest_still_waiting() -> Check {
    let mut model = Model::default();
    feed(&mut model, 1, &ask_frame("perm_x_1")?);
    feed(&mut model, 2, &ask_frame("perm_x_2")?);
    let ids = |frames: Vec<Value>| -> Vec<Value> {
        frames
            .into_iter()
            .map(|frame| frame["id"].clone())
            .collect()
    };
    let first = sent(&run(&mut model, ALLOW, Target::App, None))?;
    assert_eq!(ids(first), [json!("perm_x_1")]);
    let second = sent(&run(&mut model, ALLOW, Target::App, None))?;
    assert_eq!(
        ids(second),
        [json!("perm_x_2")],
        "the first is answered already"
    );
    Ok(())
}

#[test]
fn quitting_answers_every_ask_cancelled() -> Check {
    let mut model = asked()?;
    feed(&mut model, 2, &ask_frame("perm_x_2")?);
    let quitting = sent(&run(&mut model, CANCEL_ASKS, Target::App, None))?;
    let outcomes: Vec<&Value> = quitting.iter().map(|frame| &frame["result"]).collect();
    let cancelled = json!({"outcome": {"outcome": "cancelled"}});
    assert_eq!(outcomes, [&cancelled, &cancelled]);
    assert!(model.wire.asks.is_empty());
    Ok(())
}

/// The recorded ask with two more options a server may offer: one this app names, one it
/// does not. `yi-types` writes the kinds, so the shape is the server's own.
fn wide_ask(id: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let mut frame = ask_frame(id)?;
    let options = frame["params"]["options"]
        .as_array_mut()
        .ok_or("the ask has no options")?;
    for (option, kind) in [
        ("never", AcpPermissionOptionKind::RejectAlways),
        (
            "ask-lead",
            AcpPermissionOptionKind::Other("escalate".to_owned()),
        ),
    ] {
        let kind = serde_json::to_value(kind)?;
        options.push(json!({"optionId": option, "name": option, "kind": kind}));
    }
    Ok(frame)
}

#[test]
fn every_option_an_ask_offers_is_typed_and_a_new_kind_survives() -> Check {
    let mut model = Model::default();
    feed(&mut model, 1, &wide_ask("perm_x_1")?);
    let ask = model.wire.asks.first().ok_or("the ask was not queued")?;
    let kinds: Vec<OptionKind> = ask.offers().map(|offer| offer.kind).collect();
    assert_eq!(
        kinds,
        [
            OptionKind::AllowOnce,
            OptionKind::AllowAlways,
            OptionKind::RejectOnce,
            OptionKind::RejectAlways,
            OptionKind::Other("escalate".to_owned()),
        ]
    );
    let last = ask.offers().last().ok_or("no offers")?;
    assert_eq!((last.id.0, last.name), ("ask-lead", "ask-lead"));
    Ok(())
}

#[test]
fn any_offered_option_is_answered_by_its_id_and_a_stranger_is_not() -> Check {
    let mut model = Model::default();
    feed(&mut model, 1, &wide_ask("perm_x_1")?);
    let ask = address(Kind::Approval, "perm_x_1")?;
    let stray = sent(&run(&mut model, ANSWER, ask.clone(), Some("not-offered")))?;
    assert_eq!(stray, [Value::Null; 0], "an id the ask lacks is not sent");
    assert_eq!(model.wire.waiting().count(), 1, "and the ask still waits");
    let never = sent(&run(&mut model, ANSWER, ask, Some("never")))?;
    assert_eq!(
        never,
        [json!({"jsonrpc": "2.0", "id": "perm_x_1",
            "result": {"outcome": {"outcome": "selected", "optionId": "never"}}})]
    );
    assert_eq!(model.wire.waiting().count(), 0);
    Ok(())
}

/// The server writes the ask and the event that names its call from two places: either
/// may come first. Incident: with the event first, the Trace pane's Run it found no ask.
#[test]
fn an_ask_that_comes_after_its_event_still_names_its_call() -> Check {
    let (mut model, _) = played(&head("approval-allow.jsonl", 26)?)?;
    let rows = rows("approval-allow.jsonl")?;
    let (ask, event) = (rows.get(26), rows.get(27));
    let (Some((_, ask)), Some((_, event))) = (ask, event) else {
        return Err("the recording is shorter than its ask".into());
    };
    feed(&mut model, 1, event);
    feed(&mut model, 2, ask);
    let [ask] = model.wire.asks.as_slice() else {
        return Err(format!("{} asks are queued", model.wire.asks.len()).into());
    };
    assert_eq!(ask.tool, Some(CallId::new("c1")));
    Ok(())
}

/// ACP: a client that cancels a turn answers each ask of it `cancelled`. Incident: Stop on a
/// turn held by an ask did nothing, and the Run pressed later ran into the abort.
#[test]
fn stop_answers_the_turns_waiting_ask_cancelled() -> Check {
    let mut model = asked()?;
    let frames = sent(&run(
        &mut model,
        CANCEL,
        address(Kind::Thread, ALLOWED)?,
        None,
    ))?;
    let cancelled =
        json!({"jsonrpc": "2.0", "id": ASK, "result": {"outcome": {"outcome": "cancelled"}}});
    assert_eq!(frames.get(1), Some(&cancelled), "{frames:?}");
    assert_eq!(
        frames.first().map(|f| f["method"].clone()),
        Some(json!("session/cancel"))
    );
    assert_eq!(model.wire.waiting().count(), 0, "the card goes");
    Ok(())
}

/// After a restart only the stored answer is left: the wait counts from when the call was
/// made, or from the answer before it. Incident: a 0:21 wait read "waited 0:00".
#[test]
fn a_replayed_answer_keeps_how_long_the_ask_waited() -> Check {
    let mut entries = branch("approval-allow.jsonl")?;
    let made = entries
        .iter()
        .find(|entry| entry["message"]["role"] == "assistant")
        .and_then(|entry| entry["timestamp"].as_u64())
        .ok_or("the recording stored no call")?;
    // The recording's user answered in 12 ms; this one took 21 s.
    let answer = entries.iter_mut().find(|e| e["customType"] == "permission");
    answer.ok_or("the recording stored no answer")?["timestamp"] = json!(made + 21_000);
    let replay = json!({"sessionUpdate": "_yi/replay", "entries": entries, "from": 0});
    let mut model = Model::default();
    feed(&mut model, made + 60_000, &super::update(ALLOWED, replay));
    let gate = tool(&model, ALLOWED)?.gate.ok_or("no gate")?;
    let waited = gate.answered.map(|at| at.0.saturating_sub(gate.asked.0));
    assert_eq!(waited, Some(21_000));
    Ok(())
}
