//! Reconnects, replays and holes in the event numbers.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use yi_types::message::join_text;

use super::{Check, address, branch, entries, feed, head, played, recording, rows, run, said};
use super::{sent, thread, update};
use crate::ids::{ApprovalId, Millis};
use crate::kind::Kind;
use crate::model::Model;
use crate::msg::{Effect, Msg, step};
use crate::reduce::step::commands::OPEN;
use crate::reduce::turns::ToolState;
use crate::wire::event::{Down, WireEvent};
use crate::wire::replay::{Sync, play};

const THREAD: &str = "01a1de97-c28d-7c67-a51d-8d977b7c7930";
const ASK: &str = "perm_01a1de97-c28d-7c67-a51d-8d977b7c7930_1";
const BEFORE: [&str; 2] = ["user: run the command", "assistant: [bash c1]"];
const AFTER: [&str; 3] = [
    "user: run the command",
    "assistant: [bash c1]",
    "assistant: turn done",
];

/// The model after the first `rows` lines of `resume.jsonl`.
fn at_row(rows: usize) -> Result<Model, Box<dyn std::error::Error>> {
    Ok(played(&head("resume.jsonl", rows)?)?.0)
}

fn no_entry_twice(model: &Model) -> Check {
    let ids = entries(&thread(model, THREAD)?.transcript);
    let unique: BTreeSet<&&str> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        ids.len(),
        "an entry is in the transcript twice"
    );
    Ok(())
}

#[test]
fn a_resume_while_an_ask_waits_keeps_the_ask_and_doubles_nothing() -> Check {
    // Rows 0 to 28: the first connection, closed while the ask waits.
    let before = at_row(29)?;
    assert_eq!(said(&thread(&before, THREAD)?.transcript), BEFORE);
    // Rows 29 to 33: a second connection, `session/resume`, the replay and its reply.
    let resumed = at_row(34)?;
    let thread = thread(&resumed, THREAD)?;
    assert_eq!(
        said(&thread.transcript),
        BEFORE,
        "the replay doubled or lost"
    );
    no_entry_twice(&resumed)?;
    assert_eq!(thread.sync, Sync::Live(None));
    let ids: Vec<&ApprovalId> = resumed.wire.asks.iter().map(|ask| &ask.id).collect();
    assert_eq!(
        ids,
        [&ApprovalId::new(ASK)],
        "the server will not ask again"
    );
    let call = thread.transcript.tools.get("c1").ok_or("no call c1")?;
    assert_eq!(
        call.state,
        ToolState::Asking,
        "the call still waits on the ask"
    );
    Ok(())
}

#[test]
fn the_turn_finishes_on_the_second_connection_with_each_message_once() -> Check {
    // Rows 34 to 55: the kept id is answered and the turn runs to idle.
    let model = at_row(56)?;
    let thread = thread(&model, THREAD)?;
    assert_eq!(said(&thread.transcript), AFTER);
    no_entry_twice(&model)?;
    let call = thread.transcript.tools.get("c1").ok_or("no call c1")?;
    assert_eq!(call.state, ToolState::Done);
    assert_eq!(join_text(&call.output, "\n"), "(no output)");
    assert_eq!(call.duration(), Some(Millis(3387)));
    assert!(model.wire.asks.is_empty());
    Ok(())
}

#[test]
fn a_whole_replay_over_a_live_transcript_changes_nothing_but_the_ledger() -> Check {
    // Rows 56 to 60: a third connection replays all twelve stored entries.
    let live = at_row(56)?;
    let replayed = at_row(61)?;
    let (was, now) = (thread(&live, THREAD)?, thread(&replayed, THREAD)?);
    assert_eq!(said(&now.transcript), AFTER);
    assert_eq!(entries(&now.transcript), entries(&was.transcript));
    no_entry_twice(&replayed)?;
    assert_eq!(now.transcript.tools.len(), 1, "the call is there once");
    assert_eq!(now.title.as_deref(), Some("run the command"));
    let ledger = now
        .usage
        .ledger
        .as_ref()
        .ok_or("the replay set no totals")?;
    assert_eq!((ledger.message_count, ledger.cost_total), (4, 0.0));
    // Rows 61 to 63: a resume from where the last one ended sends no entries.
    let again = played(&recording("resume.jsonl")?)?.0;
    assert_eq!(
        thread(&again, THREAD)?.transcript,
        now.transcript,
        "an empty replay changed the transcript"
    );
    Ok(())
}

/// The recorded replay frames of `resume.jsonl`: seven entries at row 32, twelve at row 59.
fn replay_frame(row: usize) -> Result<Value, Box<dyn std::error::Error>> {
    let (_, frame) = rows("resume.jsonl")?.into_iter().nth(row).ok_or("no row")?;
    Ok(frame)
}

#[test]
fn a_replay_in_parts_adds_only_what_is_new() -> Check {
    let mut model = at_row(29)?;
    feed(&mut model, 1, &replay_frame(32)?);
    assert_eq!(said(&thread(&model, THREAD)?.transcript), BEFORE);
    // The server cuts a long branch into frames; this is the full replay's second part.
    let mut rest = replay_frame(59)?;
    let part = &mut rest["params"]["update"];
    let tail: Vec<Value> = part["entries"]
        .as_array()
        .ok_or("no entries")?
        .iter()
        .skip(7)
        .cloned()
        .collect();
    part["entries"] = Value::from(tail);
    part["from"] = Value::from(7);
    feed(&mut model, 2, &rest);
    assert_eq!(said(&thread(&model, THREAD)?.transcript), AFTER);
    feed(&mut model, 3, &rest);
    assert_eq!(said(&thread(&model, THREAD)?.transcript), AFTER, "doubled");
    no_entry_twice(&model)
}

fn resumes(effects: &[Effect]) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    Ok(sent(effects)?
        .into_iter()
        .filter(|frame| frame["method"] == "session/resume")
        .map(|frame| frame["params"].clone())
        .collect())
}

const TURN: &str = "01a1de97-bf6c-7514-9fae-b57db1eaa7e1";

#[test]
fn a_hole_in_the_event_numbers_asks_for_the_branch_again_once() -> Check {
    // Row 18 of `turn.jsonl` is event 6: leave it out, as a lagging server would.
    let holed: Vec<String> = recording("turn.jsonl")?
        .lines()
        .enumerate()
        .filter(|(row, _)| *row != 18)
        .map(|(_, line)| line.to_owned())
        .collect();
    let mut model = Model::default();
    let effects = play(&mut model, &holed.join("\n"))?;
    assert_eq!(
        resumes(&effects)?,
        [json!({"sessionId": TURN, "cwd": "$OTHER", "replayFrom": 0,
            "_meta": {"yi": {"replayUpdates": false, "shows": true}}})],
        "one resume, however many events follow the hole"
    );
    assert_eq!(thread(&model, TURN)?.sync, Sync::Resuming);

    // The branch as the server stored it: the entries of the two recorded commits.
    let stored = branch("turn.jsonl")?;
    let replay = json!({"sessionUpdate": "_yi/replay", "entries": stored, "from": 0});
    feed(&mut model, 1, &update(TURN, replay));
    let thread = thread(&model, TURN)?;
    assert_eq!(
        said(&thread.transcript),
        ["user: hello", "assistant: faux: "]
    );
    assert_eq!(thread.sync, Sync::Live(None));
    Ok(())
}

#[test]
fn a_gap_notice_or_a_swapped_worker_asks_for_the_branch_again() -> Check {
    for notice in [
        // The shapes of `gap_update` and the swap notice in Yi's `acp` crate.
        json!({"sessionUpdate": "_yi/event_gap", "seq": 11, "dropped": 3}),
        json!({"sessionUpdate": "_yi/worker_replaced", "binary": "yi", "generation": 2}),
    ] {
        let (mut model, _) = played(&recording("turn.jsonl")?)?;
        let effects = feed(&mut model, 1, &update(TURN, notice));
        assert_eq!(resumes(&effects)?.len(), 1);
        assert_eq!(thread(&model, TURN)?.sync, Sync::Resuming);
    }
    Ok(())
}

#[test]
fn a_reconnect_resumes_the_threads_a_view_shows_and_no_other() -> Check {
    let (mut model, _) = played(&recording("lists.jsonl")?)?;
    let shown = "01a1de97-d02c-79c2-940c-f7bb1e2b2cd8";
    run(&mut model, OPEN, address(Kind::Thread, shown)?, None);
    let lost = Down::Closed("the server went away".to_owned());
    step(
        &mut model,
        Msg::Wire(WireEvent::Down {
            at: Millis(5),
            why: lost,
        }),
    );
    let up = WireEvent::Up {
        at: Millis(9),
        server: "0.1041.0".to_owned(),
        latency: None,
    };
    let effects = step(&mut model, Msg::Wire(up));
    assert_eq!(
        resumes(&effects)?,
        [json!({"sessionId": shown, "cwd": "$OTHER", "replayFrom": 0,
            "_meta": {"yi": {"replayUpdates": false, "shows": true}}})]
    );
    Ok(())
}

#[test]
fn a_replay_from_the_start_replaces_a_branch_that_was_rewound() -> Check {
    let mut model = at_row(56)?;
    assert_eq!(said(&thread(&model, THREAD)?.transcript), AFTER);
    // The branch after a rewind to the prompt: the first four entries of the full replay.
    let mut rewound = replay_frame(59)?;
    let part = &mut rewound["params"]["update"];
    let kept: Vec<Value> = part["entries"]
        .as_array()
        .ok_or("no entries")?
        .iter()
        .take(4)
        .cloned()
        .collect();
    part["entries"] = Value::from(kept);
    feed(&mut model, 1, &rewound);
    let thread = thread(&model, THREAD)?;
    assert_eq!(said(&thread.transcript), ["user: run the command"]);
    assert!(
        thread.transcript.tools.is_empty(),
        "a call of the old branch outlived it"
    );
    Ok(())
}

#[test]
fn two_views_of_one_thread_make_one_resume() -> Check {
    let (mut model, _) = played(&recording("lists.jsonl")?)?;
    // A thread known only from the list: nothing of it has been replayed.
    let listed = "01a1de97-c166-7d2e-9ca8-5dee496589e1";
    let first = run(&mut model, OPEN, address(Kind::Thread, listed)?, None);
    assert_eq!(
        resumes(&first)?,
        [
            json!({"sessionId": listed, "cwd": "$PROJECT", "replayFrom": 0,
            "_meta": {"yi": {"replayUpdates": false, "shows": true}}})
        ]
    );
    let second = run(&mut model, OPEN, address(Kind::Thread, listed)?, None);
    assert_eq!(resumes(&second)?, [Value::Null; 0]);
    assert_eq!(thread(&model, listed)?.views, 2);
    // The thread the recording made is followed already: a view of it asks for nothing.
    let followed = "01a1de97-d02c-79c2-940c-f7bb1e2b2cd8";
    let third = run(&mut model, OPEN, address(Kind::Thread, followed)?, None);
    assert_eq!(resumes(&third)?, [Value::Null; 0]);
    Ok(())
}

#[test]
fn a_refused_resume_is_not_asked_again_but_a_busy_worker_is() -> Check {
    for (code, again) in [(-32602, 0), (-32000, 1)] {
        let (mut model, _) = played(&recording("lists.jsonl")?)?;
        let listed = "01a1de97-c166-7d2e-9ca8-5dee496589e1";
        let opened = sent(&run(&mut model, OPEN, address(Kind::Thread, listed)?, None))?;
        let id = opened.first().map(|frame| frame["id"].clone());
        // The shape of the refusals in `lists.jsonl`; -32000 is the server's "busy, retry".
        let refusal = json!({"jsonrpc": "2.0", "id": id.ok_or("no resume")?,
            "error": {"code": code, "message": "refused"}});
        step(&mut model, Msg::Wire(WireEvent::Line(refusal.to_string())));
        let poll = step(&mut model, Msg::Wire(WireEvent::Poll { at: Millis(9) }));
        assert_eq!(
            resumes(&poll)?.len(),
            again,
            "after a refusal with code {code}"
        );
    }
    Ok(())
}

#[test]
fn a_resume_answered_with_no_options_keeps_the_ones_known() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    run(&mut model, OPEN, address(Kind::Thread, TURN)?, None);
    let gap = json!({"sessionUpdate": "_yi/event_gap", "seq": 11, "dropped": 1});
    let asked = sent(&feed(&mut model, 1, &update(TURN, gap)))?;
    let id = asked.first().map(|frame| frame["id"].clone());
    // A daemon that has just started answers before the session is built: its options
    // come empty, as in the recorded answers to `_yi/fork`, and follow in an update.
    let answer = json!({"jsonrpc": "2.0", "id": id.ok_or("no resume")?,
        "result": {"configOptions": [], "_meta": {"yi": {"replayedTo": 6}}}});
    step(&mut model, Msg::Wire(WireEvent::Line(answer.to_string())));
    let thread = thread(&model, TURN)?;
    let mode = thread
        .option("mode")
        .map(|option| option.current_value.as_str());
    assert_eq!(mode, Some("ask"), "an empty answer wiped the options");
    assert_eq!(thread.sync, Sync::Live(None));
    Ok(())
}

/// A new app on the server of `resume.jsonl`: the thread waits on an ask asked before it
/// started, and its replay names the waiting call.
fn restarted() -> Result<Model, Box<dyn std::error::Error>> {
    let lines: Vec<String> = recording("resume.jsonl")?
        .lines()
        .map(str::to_owned)
        .collect();
    let waits = lines.get(28).ok_or("no requires_action row")?;
    let second = lines.get(29..34).ok_or("no second connection")?;
    let replayed = std::iter::once(waits).chain(second);
    Ok(played(&replayed.cloned().collect::<Vec<_>>().join("\n"))?.0)
}

#[test]
fn a_step_left_waiting_by_a_closed_app_is_lost_and_needs_nobody() -> Check {
    let model = restarted()?;
    let thread = thread(&model, THREAD)?;
    let call = thread.transcript.tools.get("c1").ok_or("no call c1")?;
    assert_eq!(call.state, ToolState::Lost);
    let id = crate::ids::ThreadId::new(THREAD);
    assert_ne!(
        model.wire.status(&id),
        crate::reduce::threads::Status::NeedsYou
    );
    assert!(
        model.wire.attention().alerts.is_empty(),
        "the bell counts it"
    );
    Ok(())
}

#[test]
fn an_ask_that_comes_after_all_wins_over_the_lost_step() -> Check {
    let mut model = restarted()?;
    let ask = rows("resume.jsonl")?
        .get(26)
        .map(|(_, frame)| frame.to_string());
    let ask = format!(r#"{{"dir":"recv","frame":{}}}"#, ask.ok_or("no ask row")?);
    play(&mut model, &ask)?;
    let call = thread(&model, THREAD)?.transcript.tools.get("c1");
    assert_eq!(call.map(|call| call.state), Some(ToolState::Asking));
    let tools: Vec<_> = model.wire.waiting().map(|ask| ask.tool.clone()).collect();
    assert_eq!(tools, [Some(crate::reduce::turns::CallId::new("c1"))]);
    Ok(())
}

/// The list rows Yi wrote for a session waiting on an ask its client never answered: the
/// daemon's own, then the project worker's, which names no state (seen from `yi serve`).
fn listed(id: u64, worker: bool) -> String {
    let meta = match worker {
        false => json!({"attached": false, "unseen": 0, "lastState": "requires_action",
            "lastEventMs": 1_794_826_801_932_u64, "pinnedMs": null}),
        true => json!({"attached": true, "createdAt": 1_794_826_801_847_u64}),
    };
    let row = json!({"sessionId": THREAD, "cwd": "$PROJECT", "title": "run the command",
        "_meta": {"yi": meta}});
    json!({"jsonrpc": "2.0", "id": id, "result": {"sessions": [row]}}).to_string()
}

/// Asks for the thread list and answers the plain list with `worker`'s row.
fn list(model: &mut Model, at: u64, worker: bool) -> Check {
    let asked = sent(&step(model, Msg::Wire(WireEvent::Poll { at: Millis(at) })))?;
    let plain = asked.iter().find(|frame| frame["params"] == json!({}));
    let id = plain
        .and_then(|frame| frame["id"].as_u64())
        .ok_or("no list asked")?;
    step(model, Msg::Wire(WireEvent::Line(listed(id, worker))));
    Ok(())
}

#[test]
fn a_project_list_that_names_no_state_leaves_a_waiting_thread_waiting() -> Check {
    let mut model = Model::default();
    let up = WireEvent::Up {
        at: Millis(1),
        server: "yi".to_owned(),
        latency: None,
    };
    step(&mut model, Msg::Wire(up));
    list(&mut model, 2, false)?;
    list(&mut model, 3, true)?;
    let id = crate::ids::ThreadId::new(THREAD);
    assert_eq!(
        model.wire.threads.get(&id).map(|thread| &thread.state),
        Some(&crate::reduce::threads::Run::NeedsYou),
        "a row with no state said the waiting run was over"
    );
    Ok(())
}

#[test]
fn an_ask_kept_when_the_app_was_killed_waits_again_after_a_restart() -> Check {
    let kept = at_row(29)?.wire.kept();
    let mut model = Model::default();
    for line in kept.lines() {
        step(&mut model, Msg::Wire(WireEvent::Line(line.to_owned())));
    }
    let lines: Vec<String> = recording("resume.jsonl")?
        .lines()
        .map(str::to_owned)
        .collect();
    let waits = lines.get(28).ok_or("no requires_action row")?;
    let second = lines.get(29..34).ok_or("no second connection")?;
    let replayed: Vec<String> = std::iter::once(waits).chain(second).cloned().collect();
    play(&mut model, &replayed.join("\n"))?;
    let call = thread(&model, THREAD)?.transcript.tools.get("c1");
    assert_eq!(call.map(|call| call.state), Some(ToolState::Asking));
    let answer = run(
        &mut model,
        crate::reduce::step::commands::ALLOW,
        ask_at()?,
        None,
    );
    let ids: Vec<Value> = sent(&answer)?
        .iter()
        .map(|frame| frame["id"].clone())
        .collect();
    assert_eq!(
        ids,
        [json!(ASK)],
        "the answer must carry the id the server asked under"
    );
    Ok(())
}

#[test]
fn a_kept_ask_whose_run_the_server_lists_as_over_is_dropped() -> Check {
    let mut model = Model::default();
    for line in at_row(29)?.wire.kept().lines() {
        step(&mut model, Msg::Wire(WireEvent::Line(line.to_owned())));
    }
    let up = WireEvent::Up {
        at: Millis(1),
        server: "yi".to_owned(),
        latency: None,
    };
    step(&mut model, Msg::Wire(up));
    let asked = sent(&step(
        &mut model,
        Msg::Wire(WireEvent::Poll { at: Millis(2) }),
    ))?;
    let id = asked
        .first()
        .and_then(|frame| frame["id"].as_u64())
        .ok_or("no list")?;
    let over = listed(id, false).replace("requires_action", "idle");
    step(&mut model, Msg::Wire(WireEvent::Line(over)));
    assert_eq!(
        model.wire.waiting().count(),
        0,
        "a card for a run that ended"
    );
    Ok(())
}

fn ask_at() -> Result<crate::command::table::Target, Box<dyn std::error::Error>> {
    address(Kind::Approval, ASK)
}
