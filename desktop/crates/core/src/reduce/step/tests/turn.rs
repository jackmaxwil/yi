//! A whole recorded turn: prompt, streamed answer, idle.

use serde_json::{Value, json};
use yi_types::acp::AcpStopReason;
use yi_types::event::{AgentEvent, RunEnd};
use yi_types::message::{AgentMessage, HostSource, UserContent};

use super::{Check, entries, feed, head, played, recording, said, thread, update};
use crate::ids::Millis;
use crate::model::Model;
use crate::msg::{Msg, step};
use crate::reduce::state::Link;
use crate::reduce::threads::{Run, Workdir};
use crate::reduce::turns::Turn;
use crate::reduce::usage::Tokens;
use crate::wire::replay::{Sync, play_line};

const THREAD: &str = "01a1de97-bf6c-7514-9fae-b57db1eaa7e1";

#[test]
fn a_recorded_turn_folds_into_the_prompt_and_its_answer() -> Check {
    let (model, _) = played(&recording("turn.jsonl")?)?;
    assert_eq!(
        model.wire.link,
        Link::Up {
            server: "0.1041.0".to_owned(),
            since: Millis(0),
            latency: None,
        }
    );
    assert_eq!(model.wire.threads.len(), 1, "one session was made");
    let thread = thread(&model, THREAD)?;
    assert_eq!(thread.cwd, "$OTHER");
    assert_eq!(
        said(&thread.transcript),
        ["user: hello", "assistant: faux: "]
    );
    assert_eq!(
        entries(&thread.transcript),
        [
            "01a1de97-c015-7830-b548-cab2c95669c6",
            "01a1de97-c03a-730c-95c5-09f641949190"
        ],
        "each message carries the entry id rewind and fork name"
    );
    assert!(
        thread.transcript.messages.iter().all(|m| m.done.is_some()),
        "a finished turn still has a message streaming"
    );
    assert_eq!(
        thread.state,
        Run::Idle {
            stop: Some(AcpStopReason::EndTurn)
        }
    );
    assert_eq!(thread.sync, Sync::Live(Some(10)));
    assert_eq!(
        thread.transcript.ended,
        Some(RunEnd::Stop),
        "why the run ended"
    );
    assert!(model.wire.asks.is_empty());
    Ok(())
}

#[test]
fn a_turn_is_one_prompt_and_what_answered_it() -> Check {
    let (model, _) = played(&recording("turn.jsonl")?)?;
    let turns = thread(&model, THREAD)?.transcript.turns();
    assert_eq!(
        turns,
        [Turn {
            number: 1,
            first: 0,
            len: 2,
            started: Millis(0),
            ended: Some(Millis(0)),
            steps: 0
        }]
    );
    Ok(())
}

#[test]
fn the_model_options_window_and_workdir_come_with_the_turn() -> Check {
    let (model, _) = played(&recording("turn.jsonl")?)?;
    let thread = thread(&model, THREAD)?;
    let values: Vec<(&str, &str)> = thread
        .options
        .iter()
        .map(|option| (option.config_id.as_str(), option.current_value.as_str()))
        .collect();
    assert_eq!(
        values,
        [
            ("model", "faux/faux-1"),
            ("thought_level", "off"),
            ("mode", "ask")
        ]
    );
    let usage = &thread.usage;
    assert_eq!((usage.used, usage.window), (Tokens(0), Tokens(128_000)));
    assert_eq!(
        thread.workdir,
        Some(Workdir {
            cwd: "$HOME/.yi/lanes/50c3946b3ed3f5c1/0".to_owned(),
            lane: Some("other".to_owned())
        })
    );
    Ok(())
}

#[test]
fn text_shows_as_it_streams_before_the_message_ends() -> Check {
    // Row 18 is the first text delta; the message ends at row 21.
    let (model, _) = played(&head("turn.jsonl", 19)?)?;
    let thread = thread(&model, THREAD)?;
    assert_eq!(
        said(&thread.transcript),
        ["user: hello", "assistant: faux: "]
    );
    let last = thread.transcript.messages.last().ok_or("no message")?;
    assert_eq!(last.done, None, "the answer is still being written");
    assert_eq!(thread.state, Run::Running);
    Ok(())
}

#[test]
fn every_message_is_stamped_when_it_arrives() -> Check {
    let mut model = Model::default();
    for (index, line) in recording("turn.jsonl")?.lines().enumerate() {
        step(&mut model, Msg::Tick(Millis(1000 + u64::try_from(index)?)));
        play_line(&mut model, line)?;
    }
    let messages = &thread(&model, THREAD)?.transcript.messages;
    let stamps: Vec<(Millis, Option<Millis>)> = messages.iter().map(|m| (m.at, m.done)).collect();
    // Rows 11 and 14 start and end the prompt; rows 16 and 21 the answer.
    assert_eq!(
        stamps,
        [
            (Millis(1011), Some(Millis(1014))),
            (Millis(1016), Some(Millis(1021)))
        ]
    );
    Ok(())
}

#[test]
fn what_the_host_says_in_a_turn_does_not_start_another() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    // A real server sends notes like these between a prompt and its answer: one in the
    // user's role and one of its own, hidden kind. No recording holds one.
    let notice = AgentMessage::host_text(HostSource::Notice, "the lane was released", 0);
    let fragment = AgentMessage::Custom {
        custom_type: "fragment".to_owned(),
        content: UserContent::Text("Permission mode: auto.".to_owned()),
        display: false,
        details: None,
        timestamp: 0,
    };
    for (seq, message) in (11..).step_by(2).zip([notice, fragment]) {
        let start = AgentEvent::MessageStart {
            message: message.clone(),
        };
        feed(&mut model, 1, &event(seq, &start)?);
        feed(
            &mut model,
            2,
            &event(seq + 1, &AgentEvent::MessageEnd { message })?,
        );
    }
    let transcript = &thread(&model, THREAD)?.transcript;
    assert_eq!(transcript.messages.len(), 4, "both notes are kept");
    let turns = transcript.turns();
    assert_eq!(turns.len(), 1, "a note opened a turn of its own");
    assert_eq!(turns.first().map(|turn| turn.len), Some(4));
    Ok(())
}

/// One agent event in the envelope the recordings show, numbered `seq`.
fn event(seq: u64, event: &AgentEvent) -> Result<Value, serde_json::Error> {
    let fields = json!({"sessionUpdate": "_yi/event",
        "event": serde_json::to_value(event)?, "seq": seq});
    Ok(update(THREAD, fields))
}
