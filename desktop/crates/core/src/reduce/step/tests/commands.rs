//! Commands into the frames the server takes, held against what a real client sent.

use serde_json::{Value, json};

use super::{Check, address, branch, feed, head, played, recording, rows, run, said, sent};
use super::{thread, update};
use crate::address::thing::Thing;
use crate::address::uri::Address;
use crate::command::table::Target;
use crate::ids::{CommandId, EntryId, Millis, ThreadId};
use crate::kind::Kind;
use crate::model::Model;
use crate::msg::{Msg, step};
use crate::reduce::state::{Asked, Failure};
use crate::reduce::step::commands::{
    CANCEL, FORK, NEW, OPEN, PIN, PROMPT, REWIND, SEEN, SET_OPTION, SETTINGS_READ, SETTINGS_WRITE,
    SLASH, STEER, message_address,
};
use crate::wire::event::{Down, WireEvent};
use crate::wire::frame::Refusal;

const LIVE: &str = "01a1de97-d02c-79c2-940c-f7bb1e2b2cd8";
const ENTRY: &str = "01a1de97-d0c8-7227-af80-0ca86e7a5f84";

/// What the recording's client sent at `row`: the method and its params.
fn recorded(row: usize) -> Result<(Value, Value), Box<dyn std::error::Error>> {
    let (_, frame) = rows("lists.jsonl")?.into_iter().nth(row).ok_or("no row")?;
    Ok((frame["method"].clone(), frame["params"].clone()))
}

/// Runs a command and gives the one call it made: the method and its params.
fn call(
    model: &mut Model,
    command: CommandId,
    target: Target,
    text: Option<&str>,
) -> Result<(Value, Value), Box<dyn std::error::Error>> {
    let frames = sent(&run(model, command, target, text))?;
    let [frame] = frames.as_slice() else {
        return Err(format!("{} frames were sent", frames.len()).into());
    };
    Ok((frame["method"].clone(), frame["params"].clone()))
}

#[test]
fn each_command_sends_what_the_recorded_client_sent() -> Check {
    // The model as it stood when the recording's client began its list of calls.
    let (mut model, _) = played(&head("lists.jsonl", 27)?)?;
    let live = address(Kind::Thread, LIVE)?;
    let message = Target::Thing(message_address(&ThreadId::new(LIVE), &EntryId::new(ENTRY))?);
    let place = address(Kind::Place, "$OTHER")?;
    let enabled = address(Kind::Setting, "catalog.enabled")?;
    let m = &mut model;
    assert_eq!(call(m, NEW, place, None)?, recorded(2)?);
    assert_eq!(
        call(m, PROMPT, live.clone(), Some("something to fork"))?,
        recorded(4)?
    );
    assert_eq!(call(m, PIN, live.clone(), None)?, recorded(29)?);
    assert_eq!(call(m, FORK, message.clone(), Some("at"))?, recorded(35)?);
    assert_eq!(call(m, FORK, message, None)?, recorded(37)?);
    assert_eq!(call(m, SETTINGS_READ, Target::App, None)?, recorded(39)?);
    assert_eq!(
        call(m, SETTINGS_WRITE, enabled, Some("true"))?,
        recorded(41)?
    );
    let option = |m: &mut Model, text| call(m, SET_OPTION, live.clone(), Some(text));
    assert_eq!(option(m, "model=faux/faux-1")?, recorded(45)?);
    assert_eq!(option(m, "thought_level=high")?, recorded(47)?);
    assert_eq!(option(m, "mode=ask")?, recorded(50)?);
    Ok(())
}

#[test]
fn seen_clears_what_was_unseen_and_tells_the_server() -> Check {
    let (mut model, _) = played(&head("lists.jsonl", 27)?)?;
    assert!(
        thread(&model, LIVE)?.unseen() > 0,
        "a turn ended with no view"
    );
    let live = address(Kind::Thread, LIVE)?;
    assert_eq!(call(&mut model, SEEN, live, None)?, recorded(31)?);
    assert_eq!(thread(&model, LIVE)?.unseen(), 0);
    Ok(())
}

#[test]
fn cancel_steer_slash_and_rewind_name_their_thread() -> Check {
    let (mut model, _) = played(&head("lists.jsonl", 27)?)?;
    let live = address(Kind::Thread, LIVE)?;
    let message = Target::Thing(message_address(&ThreadId::new(LIVE), &EntryId::new(ENTRY))?);
    let m = &mut model;
    // The methods and params of Yi's `acp` crate; no recording holds these four.
    assert_eq!(
        call(m, CANCEL, live.clone(), None)?,
        (json!("session/cancel"), json!({"sessionId": LIVE}))
    );
    assert_eq!(
        call(m, STEER, live.clone(), Some("skip the docs"))?,
        (
            json!("_yi/steer"),
            json!({"sessionId": LIVE, "text": "skip the docs"})
        )
    );
    assert_eq!(
        call(m, SLASH, live, Some("undo"))?,
        (
            json!("_yi/slash"),
            json!({"sessionId": LIVE, "line": "undo"})
        )
    );
    assert_eq!(
        call(m, REWIND, message.clone(), None)?,
        (
            json!("_yi/rewind"),
            json!({"sessionId": LIVE, "entryId": ENTRY})
        )
    );
    assert_eq!(
        call(m, REWIND, message, Some("files"))?,
        (
            json!("_yi/rewind"),
            json!({"sessionId": LIVE, "entryId": ENTRY, "files": true})
        )
    );
    Ok(())
}

#[test]
fn a_prompt_carries_the_attached_things_as_links() -> Check {
    let (mut model, _) = played(&head("lists.jsonl", 27)?)?;
    let lines = Address::new(Kind::Lines, "src/upload.rs")?.part("L13-L20");
    model.attached.things.push(Thing {
        label: "upload.rs lines 13 to 20".to_owned(),
        address: lines,
        excerpt: Some("fn upload() {}".to_owned()),
    });
    let (_, params) = call(
        &mut model,
        PROMPT,
        address(Kind::Thread, LIVE)?,
        Some("why?"),
    )?;
    assert_eq!(
        params["prompt"],
        json!([
            {"type": "text", "text": "why?"},
            {"type": "resource_link", "uri": "src/upload.rs#L13-L20",
                "name": "upload.rs lines 13 to 20"},
            {"type": "text", "text": "\nfn upload() {}\n"}
        ])
    );
    Ok(())
}

/// The server's answer at `row` of `name`, under the id a request here was sent with.
fn answer(name: &str, row: usize, id: &Value) -> Result<WireEvent, Box<dyn std::error::Error>> {
    let (_, mut frame) = rows(name)?.into_iter().nth(row).ok_or("no row")?;
    frame["id"] = id.clone();
    Ok(WireEvent::Line(frame.to_string()))
}

#[test]
fn a_new_thread_gets_its_first_prompt_once_the_server_has_made_it() -> Check {
    let mut model = Model::default();
    let ticket = model.wire.ticket();
    let asked = sent(&run(
        &mut model,
        NEW,
        address(Kind::Place, "$OTHER")?,
        Some("hello"),
    ))?;
    let id = asked
        .first()
        .map(|frame| frame["id"].clone())
        .ok_or("nothing sent")?;
    assert!(
        model.wire.born(&ticket).is_none(),
        "the server has not answered"
    );
    // Row 3 of `turn.jsonl` is the server's answer to `session/new`.
    let effects = step(&mut model, Msg::Wire(answer("turn.jsonl", 3, &id)?));
    let made = model.wire.born(&ticket).ok_or("the thread was not made")?;
    assert_eq!(
        made.id,
        ThreadId::new("01a1de97-bf6c-7514-9fae-b57db1eaa7e1")
    );
    assert_eq!(made.cwd, "$OTHER");
    let prompts: Vec<Value> = sent(&effects)?
        .into_iter()
        .map(|f| f["params"].clone())
        .collect();
    let (_, first) = rows("turn.jsonl")?.into_iter().nth(4).ok_or("no row 4")?;
    assert_eq!(
        prompts,
        [first["params"].clone()],
        "the prompt of the recording"
    );
    Ok(())
}

#[test]
fn a_prompt_the_server_refuses_goes_back_to_the_composer() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    let id = "01a1de97-bf6c-7514-9fae-b57db1eaa7e1";
    let ticket = model.wire.ticket();
    let frames = sent(&run(
        &mut model,
        PROMPT,
        address(Kind::Thread, id)?,
        Some("and then?"),
    ))?;
    let asked: Vec<&str> = model.wire.queued(&ThreadId::new(id)).collect();
    assert_eq!(
        asked,
        ["and then?"],
        "a prompt waits until the server takes it"
    );
    let call = frames
        .first()
        .map(|frame| frame["id"].clone())
        .ok_or("nothing sent")?;
    // The shape of the refusals in `lists.jsonl`.
    let refusal = json!({"jsonrpc": "2.0", "id": call,
        "error": {"code": -32603, "message": "the worker is gone"}});
    step(&mut model, Msg::Wire(WireEvent::Line(refusal.to_string())));
    assert_eq!(thread(&model, id)?.prefill.as_deref(), Some("and then?"));
    let failure = model
        .wire
        .failed(&ticket)
        .ok_or("the refusal was not kept")?;
    assert!(matches!(failure.asked, Asked::Prompt { .. }));
    assert_eq!(
        failure.why,
        Refusal::Server {
            code: -32603,
            message: "the worker is gone".to_owned()
        }
    );
    assert_eq!(model.wire.queued(&ThreadId::new(id)).count(), 0);
    Ok(())
}

#[test]
fn a_request_that_never_left_fails_as_unsent() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    let ticket = model.wire.ticket();
    let id = "01a1de97-bf6c-7514-9fae-b57db1eaa7e1";
    run(
        &mut model,
        PROMPT,
        address(Kind::Thread, id)?,
        Some("offline"),
    );
    let unsent = WireEvent::Unsent {
        at: Millis(3),
        id: ticket.clone(),
    };
    step(&mut model, Msg::Wire(unsent));
    assert_eq!(
        model.wire.failed(&ticket).map(|f| &f.why),
        Some(&Refusal::Unsent)
    );
    assert_eq!(thread(&model, id)?.prefill.as_deref(), Some("offline"));
    Ok(())
}

const TURN: &str = "01a1de97-bf6c-7514-9fae-b57db1eaa7e1";

/// The entry that stored the recorded turn's prompt, as the server writes one for `words`.
fn stored(entry: &str, words: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let mut entries = branch("turn.jsonl")?.into_iter();
    let prompt = entries.find(|entry| entry["message"]["role"] == "user");
    let mut stored = prompt.ok_or("the recording stored no prompt")?;
    stored["id"] = Value::from(entry);
    stored["message"]["content"] = Value::from(words);
    Ok(stored)
}

/// The recorded turn's whole branch and `more`, as the server replays it after a resume.
fn replayed(more: Vec<Value>) -> Result<Value, Box<dyn std::error::Error>> {
    let mut entries = branch("turn.jsonl")?;
    entries.extend(more);
    let replay = json!({"sessionUpdate": "_yi/replay", "entries": entries, "from": 0});
    Ok(update(TURN, replay))
}

fn queued(model: &Model) -> Vec<&str> {
    model.wire.queued(&ThreadId::new(TURN)).collect()
}

/// The connection goes and comes back while a view shows the recorded turn's thread.
fn reconnect(model: &mut Model) {
    let lost = WireEvent::Down {
        at: Millis(4),
        why: Down::Closed("the server went away".to_owned()),
    };
    step(model, Msg::Wire(lost));
    let up = WireEvent::Up {
        at: Millis(5),
        server: "0.1041.0".to_owned(),
        latency: None,
    };
    step(model, Msg::Wire(up));
}

#[test]
fn prompts_lost_with_the_connection_wait_until_the_server_stores_them() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    run(&mut model, OPEN, address(Kind::Thread, TURN)?, None);
    // "hello" is what the recorded turn began with: the user says it again.
    for text in ["hello", "second thought"] {
        run(&mut model, PROMPT, address(Kind::Thread, TURN)?, Some(text));
    }
    reconnect(&mut model);
    assert_eq!(
        queued(&model),
        ["hello", "second thought"],
        "the server may hold them"
    );
    assert_eq!(
        thread(&model, TURN)?.prefill,
        None,
        "sent again from the composer, a prompt the server holds runs twice"
    );
    let prompt = |failure: &&Failure| matches!(failure.asked, Asked::Prompt { .. });
    assert_eq!(
        model.wire.failures.iter().find(prompt),
        None,
        "a prompt whose answer was lost has not failed"
    );

    // The branch as it stood when the connection went: its "hello" is the old one.
    feed(&mut model, 6, &replayed(Vec::new())?);
    assert_eq!(
        queued(&model),
        ["hello", "second thought"],
        "a message stored before the prompt was sent took it"
    );

    // The server had read the first and stores it; the second it never got.
    let again = stored("01a1de97-ffff-7000-8000-00000000000a", "hello")?;
    let commit = json!({"sessionUpdate": "_yi/committed", "ancestors": [], "entry": again});
    feed(&mut model, 7, &update(TURN, commit));
    assert_eq!(queued(&model), ["second thought"]);
    let thread = thread(&model, TURN)?;
    assert_eq!(
        said(&thread.transcript),
        ["user: hello", "assistant: faux: ", "user: hello"]
    );
    assert_eq!(thread.prefill, None);
    Ok(())
}

#[test]
fn one_stored_message_lets_go_of_one_of_two_lost_prompts_alike() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    run(&mut model, OPEN, address(Kind::Thread, TURN)?, None);
    for _ in 0..2 {
        run(
            &mut model,
            PROMPT,
            address(Kind::Thread, TURN)?,
            Some("again"),
        );
    }
    reconnect(&mut model);
    let entry = stored("01a1de97-ffff-7000-8000-00000000000c", "again")?;
    // The server stored one; the branch is then replayed whole, with it, a second time.
    feed(&mut model, 6, &replayed(vec![entry.clone()])?);
    feed(&mut model, 7, &replayed(vec![entry])?);
    assert_eq!(
        queued(&model),
        ["again"],
        "the second may still be on its way"
    );
    Ok(())
}

#[test]
fn a_lost_prompt_with_things_attached_is_known_by_the_words_it_starts_with() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    run(&mut model, OPEN, address(Kind::Thread, TURN)?, None);
    let lines = Address::new(Kind::Lines, "src/upload.rs")?.part("L13-L20");
    model.attached.things.push(Thing {
        label: "upload.rs lines 13 to 20".to_owned(),
        address: lines,
        excerpt: Some("fn upload() {}".to_owned()),
    });
    run(
        &mut model,
        PROMPT,
        address(Kind::Thread, TURN)?,
        Some("why?"),
    );
    reconnect(&mut model);
    assert_eq!(queued(&model), ["why?"]);
    // `prompt_parts` in Yi's `acp` crate stores a prompt's blocks as one text: each text as
    // it is, a link's address after a space. No recording holds a link.
    let words = "why? yi://lines/src/upload.rs#L13-L20\nfn upload() {}\n";
    let entry = stored("01a1de97-ffff-7000-8000-00000000000b", words)?;
    feed(&mut model, 6, &replayed(vec![entry])?);
    assert_eq!(queued(&model), [""; 0], "the server stored it");
    Ok(())
}

/// Runs a command on the recorded thread and steps the server's `result` for it.
fn answered(
    command: CommandId,
    target: Target,
    text: &str,
    result: Value,
) -> Result<Model, Box<dyn std::error::Error>> {
    let (mut model, _) = played(&head("lists.jsonl", 27)?)?;
    let frames = sent(&run(&mut model, command, target, Some(text)))?;
    let id = frames.first().map(|frame| frame["id"].clone());
    let reply = json!({"jsonrpc": "2.0", "id": id.ok_or("nothing sent")?, "result": result});
    step(&mut model, Msg::Wire(WireEvent::Line(reply.to_string())));
    Ok(model)
}

#[test]
fn what_a_slash_command_answers_is_kept_as_a_note() -> Check {
    // What Yi's own protocol test reads back from `/undo` on a thread with no turn.
    let words = "/undo: this session has taken no turn yet — `yi undo` restores an earlier session";
    let model = answered(
        SLASH,
        address(Kind::Thread, LIVE)?,
        "undo",
        json!({"text": words}),
    )?;
    let notes = &thread(&model, LIVE)?.notes;
    assert_eq!(notes.last().map(|note| note.text.as_str()), Some(words));
    Ok(())
}

#[test]
fn a_rewind_puts_the_prompt_it_took_back_in_the_composer() -> Check {
    let message = Target::Thing(message_address(&ThreadId::new(LIVE), &EntryId::new(ENTRY))?);
    // The shape Yi's `acp` crate gives `_yi/rewind`; `unsent` is the prompt it undid.
    let result = json!({"leafId": "01a1de97-d0c7-783f-a543-626bf8dbdd06",
        "unsent": "something to fork", "summarizing": false, "restored": null});
    let model = answered(REWIND, message, "", result)?;
    assert_eq!(
        thread(&model, LIVE)?.prefill.as_deref(),
        Some("something to fork")
    );
    Ok(())
}

/// Incident: a prompt sent while a turn ran was held by Yi as a follow-up and never answered.
#[test]
fn a_prompt_typed_while_the_turn_runs_goes_once_the_turn_ends() -> Check {
    // The recorded turn after the server took its prompt: it is running.
    let (mut model, _) = played(&head("turn.jsonl", 14)?)?;
    let early = sent(&run(
        &mut model,
        PROMPT,
        address(Kind::Thread, TURN)?,
        Some("and the docs"),
    ))?;
    assert_eq!(early, [Value::Null; 0], "nothing goes while the turn runs");
    assert_eq!(queued(&model), ["and the docs"]);
    let idle = json!({"sessionUpdate": "state_update", "state": "idle", "stopReason": "end_turn"});
    let frames = sent(&feed(&mut model, 9, &update(TURN, idle)))?;
    let words = |frame: &Value| frame["params"]["prompt"][0]["text"].clone();
    let [frame] = frames.as_slice() else {
        return Err(format!("{} frames went as the turn ended", frames.len()).into());
    };
    assert_eq!(
        (frame["method"].clone(), words(frame)),
        (json!("session/prompt"), json!("and the docs"))
    );
    Ok(())
}
