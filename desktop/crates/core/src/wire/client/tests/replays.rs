//! Every recording, replayed through the client's threads and the reducer.

use yi_types::message::join_text;

use super::{Check, Plan, Rig};
use crate::command::table::Target;
use crate::ids::{ApprovalId, Millis, ThreadId};
use crate::kind::Kind;
use crate::model::Model;
use crate::reduce::state::Link;
use crate::reduce::step::commands::{
    ALLOW, DENY, FORK, NEW, OPEN, PIN, PROMPT, SEEN, SET_OPTION, SETTINGS_READ, SETTINGS_WRITE,
};
use crate::reduce::step::tests::{address, entries, rows, said};
use crate::reduce::threads::{Run, Thread};
use crate::reduce::turns::ToolState;
use crate::wire::event::Down;
use crate::wire::frame::{Kept, MAX_FRAME};
use crate::wire::replay::Sync;

fn find<'a>(model: &'a Model, id: &str) -> Option<&'a Thread> {
    model.wire.threads.get(&ThreadId::new(id))
}

fn idle(model: &Model, id: &str) -> bool {
    find(model, id).is_some_and(|thread| matches!(thread.state, Run::Idle { stop: Some(_) }))
}

fn the<'a>(model: &'a Model, id: &str) -> Result<&'a Thread, Box<dyn std::error::Error>> {
    find(model, id).ok_or_else(|| format!("no thread {id}").into())
}

/// Connects, makes a thread in `cwd` as the recordings do, and shows it in a view.
fn begin(rig: &mut Rig, cwd: &str, id: &str) -> Check {
    rig.until("the server answers initialize", |model| {
        matches!(model.wire.link, Link::Up { .. })
    })?;
    rig.run(NEW, address(Kind::Place, cwd)?, None)?;
    rig.until("the thread is made", |model| find(model, id).is_some())?;
    let made = Target::Thing(the(&rig.model, id)?.address()?);
    rig.run(OPEN, made, None)
}

#[test]
fn a_recorded_turn_through_the_socket_folds_into_the_model() -> Check {
    let id = "01a1de97-bf6c-7514-9fae-b57db1eaa7e1";
    let mut rig = Rig::start(rows("turn.jsonl")?, Plan::default())?;
    begin(&mut rig, "$OTHER", id)?;
    rig.run(PROMPT, address(Kind::Thread, id)?, Some("hello"))?;
    rig.until("the turn ends", |model| idle(model, id))?;
    let thread = the(&rig.model, id)?;
    assert_eq!(
        said(&thread.transcript),
        ["user: hello", "assistant: faux: "]
    );
    assert_eq!(
        entries(&thread.transcript),
        [
            "01a1de97-c015-7830-b548-cab2c95669c6",
            "01a1de97-c03a-730c-95c5-09f641949190"
        ]
    );
    assert_eq!(thread.sync, Sync::Live(Some(10)));
    let stamps: Vec<Millis> = thread.transcript.messages.iter().map(|m| m.at).collect();
    assert!(
        stamps.windows(2).all(|pair| pair[0] < pair[1]) && stamps.first() > Some(&Millis(1_000)),
        "messages are stamped by the client's clock as they arrive: {stamps:?}"
    );
    let Link::Up { latency, .. } = &rig.model.wire.link else {
        return Err("the link is not up".into());
    };
    assert!(
        latency.is_some_and(|took| took < Millis(10_000)),
        "the handshake is timed: {latency:?}"
    );
    rig.finish()
}

#[test]
fn a_line_that_is_no_frame_costs_neither_the_connection_nor_the_turn() -> Check {
    let id = "01a1de97-bf6c-7514-9fae-b57db1eaa7e1";
    // Row 17 is in the middle of the streamed answer. One line is short; one is a byte
    // longer than a frame may be, which the reader skips without holding it.
    for len in [12, usize::try_from(MAX_FRAME)?.saturating_add(1)] {
        let plan = Plan {
            noise: Some((17, len)),
            ..Plan::default()
        };
        let mut rig = Rig::start(rows("turn.jsonl")?, plan)?;
        begin(&mut rig, "$OTHER", id)?;
        rig.run(PROMPT, address(Kind::Thread, id)?, Some("hello"))?;
        rig.until("the turn ends", |model| idle(model, id))?;
        assert_eq!(rig.model.wire.dropped, 1, "a line of {len} bytes");
        assert!(
            matches!(rig.model.wire.link, Link::Up { .. }),
            "a line of {len} bytes cost the connection"
        );
        assert_eq!(
            said(&the(&rig.model, id)?.transcript),
            ["user: hello", "assistant: faux: "]
        );
        rig.finish()?;
    }
    Ok(())
}

/// Plays an approval recording up to its ask, answers with `command`, and runs it to idle.
fn approval(
    name: &str,
    id: &str,
    command: crate::ids::CommandId,
) -> Result<Rig, Box<dyn std::error::Error>> {
    let mut rig = Rig::start(rows(name)?, Plan::default())?;
    begin(&mut rig, "$PROJECT", id)?;
    rig.run(PROMPT, address(Kind::Thread, id)?, Some("run the command"))?;
    rig.until("the ask arrives", |model| {
        model.wire.asks.len() == 1 && find(model, id).is_some_and(|t| t.state == Run::NeedsYou)
    })?;
    let ask = rig.model.wire.asks.first().ok_or("no ask")?;
    assert_eq!(ask.id, ApprovalId::new(format!("perm_{id}_1")));
    let ask = Target::Thing(ask.address()?);
    rig.run(command, ask, None)?;
    rig.until("the turn ends", |model| idle(model, id))?;
    assert!(rig.model.wire.asks.is_empty(), "the answered ask is gone");
    Ok(rig)
}

#[test]
fn an_ask_allowed_through_the_socket_runs_the_command() -> Check {
    let id = "01a1de97-c166-7d2e-9ca8-5dee496589e1";
    let rig = approval("approval-allow.jsonl", id, ALLOW)?;
    let thread = the(&rig.model, id)?;
    let call = thread.transcript.tools.get("c1").ok_or("no call c1")?;
    assert_eq!(call.state, ToolState::Done);
    assert_eq!(join_text(&call.output, "\n"), "(no output)");
    assert_eq!(call.duration(), Some(Millis(73)));
    assert_eq!(
        said(&thread.transcript),
        [
            "user: run the command",
            "assistant: [bash c1]",
            "assistant: turn done"
        ]
    );
    rig.finish()
}

#[test]
fn an_ask_denied_through_the_socket_fails_the_command() -> Check {
    let id = "01a1de97-c223-744c-8afa-018c5c9ad664";
    let rig = approval("approval-reject.jsonl", id, DENY)?;
    let call = the(&rig.model, id)?
        .transcript
        .tools
        .get("c1")
        .ok_or("no call c1")?;
    assert_eq!(call.state, ToolState::Failed);
    assert_eq!(call.gate.and_then(|gate| gate.allowed), Some(false));
    rig.finish()
}

const RESUMED: &str = "01a1de97-c28d-7c67-a51d-8d977b7c7930";
const BEFORE: [&str; 2] = ["user: run the command", "assistant: [bash c1]"];
const AFTER: [&str; 3] = [
    "user: run the command",
    "assistant: [bash c1]",
    "assistant: turn done",
];

fn replayed(model: &Model, messages: u64) -> bool {
    let ledger = find(model, RESUMED).and_then(|thread| thread.usage.ledger.as_ref());
    ledger.is_some_and(|ledger| ledger.message_count == messages)
        && matches!(model.wire.link, Link::Up { .. })
}

#[test]
fn a_dropped_connection_resumes_and_nothing_is_lost_or_doubled() -> Check {
    // The recording's client hung up after rows 28 and 55; here the server does, and the
    // last three rows are a resume from an offset, which this client never asks for.
    let plan = Plan {
        cut: &[28, 55],
        ..Plan::default()
    };
    let recorded: Vec<_> = rows("resume.jsonl")?.into_iter().take(61).collect();
    let mut rig = Rig::start(recorded, plan)?;
    begin(&mut rig, "$PROJECT", RESUMED)?;
    rig.run(
        PROMPT,
        address(Kind::Thread, RESUMED)?,
        Some("run the command"),
    )?;

    rig.until("the second connection has replayed", |m| replayed(m, 2))?;
    let ask = ApprovalId::new(format!("perm_{RESUMED}_1"));
    assert_eq!(said(&the(&rig.model, RESUMED)?.transcript), BEFORE);
    let kept: Vec<&ApprovalId> = rig.model.wire.asks.iter().map(|ask| &ask.id).collect();
    assert_eq!(kept, [&ask], "the ask outlived the connection it came on");

    rig.run(ALLOW, address(Kind::Approval, ask.as_str())?, None)?;
    rig.until("the turn ends", |model| idle(model, RESUMED))?;
    assert_eq!(said(&the(&rig.model, RESUMED)?.transcript), AFTER);

    rig.until("the third connection has replayed", |m| replayed(m, 4))?;
    let thread = the(&rig.model, RESUMED)?;
    assert_eq!(
        said(&thread.transcript),
        AFTER,
        "a full replay doubled or lost"
    );
    assert_eq!(thread.transcript.tools.len(), 1);
    assert_eq!(entries(&thread.transcript).len(), 3);
    rig.finish()
}

#[test]
fn a_prompt_the_server_took_before_hanging_up_is_not_given_back_to_send_again() -> Check {
    // Rows 0 to 4 end with the prompt: the server reads it and hangs up before its answer.
    // Rows 29 to 33 are the next connection: the resume, the branch replayed, the answer.
    let recorded = rows("resume.jsonl")?;
    let (first, next) = (recorded.iter().take(5), recorded.iter().skip(29).take(5));
    let plan = Plan {
        cut: &[4],
        ..Plan::default()
    };
    let mut rig = Rig::start(first.chain(next).cloned().collect(), plan)?;
    begin(&mut rig, "$PROJECT", RESUMED)?;
    rig.run(
        PROMPT,
        address(Kind::Thread, RESUMED)?,
        Some("run the command"),
    )?;

    rig.until("the second connection has replayed", |m| replayed(m, 2))?;
    let thread = the(&rig.model, RESUMED)?;
    assert_eq!(
        said(&thread.transcript),
        BEFORE,
        "the server ran the prompt it took"
    );
    assert_eq!(
        thread.prefill, None,
        "a prompt in the transcript is in the composer too: sent again it runs twice"
    );
    let waiting = rig.model.wire.queued(&ThreadId::new(RESUMED)).count();
    assert_eq!(waiting, 0, "a prompt in the transcript still waits");
    rig.finish()
}

#[test]
fn a_frame_cut_short_is_never_folded_and_the_thread_resumes() -> Check {
    // Row 28 says the thread needs the user; the server sends half of it and hangs up.
    let plan = Plan {
        torn: Some(28),
        gate: &[29],
        ..Plan::default()
    };
    let recorded: Vec<_> = rows("resume.jsonl")?.into_iter().take(34).collect();
    let mut rig = Rig::start(recorded, plan)?;
    begin(&mut rig, "$PROJECT", RESUMED)?;
    rig.run(
        PROMPT,
        address(Kind::Thread, RESUMED)?,
        Some("run the command"),
    )?;
    rig.until("the socket closes", |model| {
        matches!(model.wire.link, Link::Down { .. })
    })?;
    let Link::Down { why, .. } = &rig.model.wire.link else {
        return Err("the link is not down".into());
    };
    assert!(
        matches!(why, Down::Closed(text) if text.contains("middle of a frame")),
        "{why}"
    );
    let thread = the(&rig.model, RESUMED)?;
    assert_eq!(thread.state, Run::Running, "half a frame changed the model");
    assert_eq!(
        rig.model.wire.dropped, 0,
        "half a frame was read as a bad one"
    );
    assert_eq!(
        rig.model.wire.asks.len(),
        1,
        "the whole frames before it count"
    );

    rig.go()?;
    rig.until("the thread has replayed", |m| replayed(m, 2))?;
    assert_eq!(said(&the(&rig.model, RESUMED)?.transcript), BEFORE);
    rig.finish()
}

const LIVE: &str = "01a1de97-d02c-79c2-940c-f7bb1e2b2cd8";
const ENTRY: &str = "01a1de97-d0c8-7227-af80-0ca86e7a5f84";
const FORK_AT: &str = "01a1de97-d0eb-7301-b580-7b77cdcfaafe";
const FORK_BEFORE: &str = "01a1de97-d0f4-7e36-a832-36e738963e34";

fn answered(rig: &mut Rig, what: &str) -> Check {
    rig.until(what, |model| model.wire.pending.is_empty())
}

#[test]
fn the_lists_recording_through_the_socket_gives_threads_pins_forks_and_settings() -> Check {
    // Three calls of the recording probe methods this app has no command for.
    let plan = Plan {
        skip: &["_yi/branch_diff", "_yi/workdir", "_yi/no_such_method"],
        ..Plan::default()
    };
    let mut rig = Rig::start(rows("lists.jsonl")?, plan)?;
    begin(&mut rig, "$OTHER", LIVE)?;
    let live = address(Kind::Thread, LIVE)?;
    let message = address(Kind::Message, &format!("{LIVE}/{ENTRY}"))?;
    rig.run(PROMPT, live.clone(), Some("something to fork"))?;
    rig.until("the turn ends", |model| idle(model, LIVE))?;

    rig.poll()?;
    rig.until("the list arrives", |model| model.wire.threads.len() == 6)?;
    rig.run(PIN, live.clone(), None)?;
    rig.until("the pin is kept", |model| {
        find(model, LIVE).is_some_and(|thread| thread.pinned == Some(Millis(1_794_735_853_802)))
    })?;
    rig.run(SEEN, live.clone(), None)?;
    answered(&mut rig, "seen is answered")?;
    rig.poll()?;
    answered(&mut rig, "the second list arrives")?;
    assert_eq!(
        rig.model.wire.listed().first().map(|t| t.id.as_str()),
        Some(LIVE)
    );

    rig.run(FORK, message.clone(), Some("at"))?;
    rig.until("the fork exists", |model| find(model, FORK_AT).is_some())?;
    rig.run(FORK, message, None)?;
    rig.until("the second fork exists", |model| {
        find(model, FORK_BEFORE).is_some()
    })?;

    rig.run(SETTINGS_READ, Target::App, None)?;
    rig.until("the settings arrive", |model| {
        model.wire.settings.len() == 67
    })?;
    rig.run(
        SETTINGS_WRITE,
        address(Kind::Setting, "catalog.enabled")?,
        Some("true"),
    )?;
    answered(&mut rig, "the write is answered")?;
    rig.run(
        SETTINGS_WRITE,
        address(Kind::Setting, "no.such.key")?,
        Some("true"),
    )?;
    rig.until("the write is refused", |model| {
        model.wire.failures.len() == 1
    })?;
    for (option, failures) in [
        ("model=faux/faux-1", 2),
        ("thought_level=high", 2),
        ("mode=ask", 2),
        ("mode=nope", 3),
    ] {
        rig.run(SET_OPTION, live.clone(), Some(option))?;
        rig.until(option, |model| {
            model.wire.pending.is_empty() && model.wire.failures.len() == failures
        })?;
    }
    rig.until("the forks' own updates arrive", |model| {
        find(model, FORK_BEFORE).is_some_and(|thread| thread.workdir.is_some())
    })?;

    let model = &rig.model;
    assert_eq!(model.wire.threads.len(), 8);
    assert_eq!(the(model, FORK_AT)?.forked_from, Some(ThreadId::new(LIVE)));
    let prefill = the(model, FORK_BEFORE)?.prefill.as_deref();
    assert_eq!(prefill, Some("something to fork"));
    let enabled = model.wire.settings.iter().filter_map(Kept::known);
    let enabled = enabled.clone().find(|row| row.id == "catalog.enabled");
    assert_eq!(
        enabled.map(|row| &row.value),
        Some(&serde_json::json!(true))
    );
    assert_eq!(said(&the(model, LIVE)?.transcript).len(), 2);
    rig.finish()
}
