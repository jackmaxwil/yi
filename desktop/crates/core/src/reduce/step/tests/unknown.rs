//! A server newer than this app: nothing it sends may break the fold.

use serde_json::{Value, json};

use super::{Check, feed, head, played, recording, rows, said, sent, thread, update};
use crate::model::Model;
use crate::msg::{Msg, step};
use crate::reduce::threads::{MAX_THREADS, Run};
use crate::reduce::turns::Body;
use crate::wire::event::WireEvent;
use crate::wire::replay::Sync;
use crate::wire::shapes::AcpStopReason;

const THREAD: &str = "01a1de97-bf6c-7514-9fae-b57db1eaa7e1";

#[test]
fn an_event_kind_newer_than_this_app_is_kept_and_breaks_nothing() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    let before = said(&thread(&model, THREAD)?.transcript);
    // The envelope of a recorded event; `AgentEvent` has no case for this type.
    let event = json!({"type": "brand_new", "payload": {"n": 1}});
    let frame = json!({"sessionUpdate": "_yi/event", "event": event, "seq": 11});
    let effects = feed(&mut model, 1, &update(THREAD, frame));
    assert_eq!(
        sent(&effects)?,
        [Value::Null; 0],
        "an unknown kind is no hole"
    );
    let thread = thread(&model, THREAD)?;
    assert_eq!(said(&thread.transcript), before);
    assert_eq!(thread.transcript.unread, [event], "its raw value is kept");
    assert_eq!(thread.sync, Sync::Live(Some(11)), "its number still counts");
    Ok(())
}

#[test]
fn an_update_kind_newer_than_this_app_is_kept_by_its_name() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    let frame = json!({"sessionUpdate": "_yi/brand_new", "x": 1});
    feed(&mut model, 1, &update(THREAD, frame));
    let kept = thread(&model, THREAD)?.extras.get("_yi/brand_new");
    assert_eq!(kept, Some(&json!({"x": 1})));
    Ok(())
}

#[test]
fn unknown_fields_and_values_on_a_known_update_still_fold() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    let later = json!({"sessionUpdate": "state_update", "state": "idle",
        "stopReason": "_yi/later", "futureField": [1, 2]});
    feed(&mut model, 1, &update(THREAD, later));
    assert_eq!(
        thread(&model, THREAD)?.state,
        Run::Idle {
            stop: Some(AcpStopReason::Other("_yi/later".to_owned()))
        }
    );
    let paused = json!({"sessionUpdate": "state_update", "state": "paused"});
    feed(&mut model, 2, &update(THREAD, paused));
    assert_eq!(
        thread(&model, THREAD)?.state,
        Run::Other("paused".to_owned())
    );
    Ok(())
}

#[test]
fn a_line_that_is_no_frame_is_counted_and_dropped() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    let before = model.wire.clone();
    for line in ["{\"jsonrpc\": \"2.0\", \"meth", "[1, 2]", ""] {
        step(&mut model, Msg::Wire(WireEvent::Line(line.to_owned())));
    }
    assert_eq!(model.wire.dropped, 3);
    assert_eq!(model.wire.threads, before.threads);
    Ok(())
}

#[test]
fn a_method_this_app_does_not_have_is_refused_so_the_server_does_not_wait() -> Check {
    let mut model = Model::default();
    // A client method of the protocol this app advertises no capability for.
    let call = json!({"jsonrpc": "2.0", "id": "fs_1", "method": "fs/read_text_file",
        "params": {"path": "/etc/passwd"}});
    let answers = sent(&feed(&mut model, 1, &call))?;
    let [answer] = answers.as_slice() else {
        return Err(format!("{} answers", answers.len()).into());
    };
    assert_eq!(answer["id"], "fs_1");
    assert_eq!(answer["error"]["code"], -32601);
    Ok(())
}

#[test]
fn a_server_cannot_make_more_threads_than_the_cap() -> Check {
    let mut model = Model::default();
    for n in 0..=MAX_THREADS {
        let name = json!({"sessionUpdate": "_yi/name", "name": "one more"});
        feed(&mut model, 1, &update(&format!("s{n}"), name));
    }
    assert_eq!(model.wire.threads.len(), MAX_THREADS);
    Ok(())
}

#[test]
fn an_entry_of_a_kind_newer_than_this_app_is_kept_in_its_place() -> Check {
    let (mut model, _) = played(&head("resume.jsonl", 29)?)?;
    let id = "01a1de97-c28d-7c67-a51d-8d977b7c7930";
    // Row 59 replays twelve entries. Entry 3 is the prompt and entry 10 the last answer:
    // one gets a role, the other a type, that no build knows yet.
    let (_, mut replay) = rows("resume.jsonl")?.into_iter().nth(59).ok_or("no row")?;
    let entries = &mut replay["params"]["update"]["entries"];
    entries[3]["message"]["role"] = Value::from("narrator");
    entries[10]["type"] = Value::from("brand_new");
    feed(&mut model, 1, &replay);
    let messages = &thread(&model, id)?.transcript.messages;
    let kinds: Vec<&str> = messages
        .iter()
        .map(|message| match &message.body {
            Body::Message(_) => "read",
            Body::Raw(_) => "raw",
        })
        .collect();
    assert_eq!(
        kinds,
        ["raw", "read", "raw"],
        "kept, and where the server put them"
    );
    let Some(Body::Raw(first)) = messages.first().map(|message| &message.body) else {
        return Err("the first is not raw".into());
    };
    assert_eq!(first["message"]["content"], "run the command");
    assert_eq!(said(&thread(&model, id)?.transcript).len(), 3);
    Ok(())
}
