//! The polled thread list, pins, forks and Yi's settings, from `lists.jsonl`.

use serde_json::{Value, json};
use yi_types::acp::AcpStopReason;
use yi_types::settings::SettingRow;

use super::{Check, head, played, recording, said, sent, thread};
use crate::ids::{Millis, ThreadId};
use crate::model::Model;
use crate::msg::{Effect, Msg, step};
use crate::reduce::agents::MAX_AGENTS;
use crate::reduce::state::{Asked, Link, MAX_FAILURES};
use crate::reduce::threads::{MAX_EXTRAS, MAX_NOTES, Run, Workdir};
use crate::reduce::turns::MAX_UNREAD;
use crate::wire::event::WireEvent;
use crate::wire::frame::{Kept, Refusal};
use crate::wire::replay::Sync;

/// The thread the recording's client made and prompted.
const LIVE: &str = "01a1de97-d02c-79c2-940c-f7bb1e2b2cd8";
/// Sessions of the earlier recordings, known only from the list.
const LISTED: [&str; 5] = [
    "01a1de97-c28d-7c67-a51d-8d977b7c7930",
    "01a1de97-c054-7f61-9c79-55bbee40a332",
    "01a1de97-c166-7d2e-9ca8-5dee496589e1",
    "01a1de97-bf6c-7514-9fae-b57db1eaa7e1",
    "01a1de97-c223-744c-8afa-018c5c9ad664",
];
const FORK_AT: &str = "01a1de97-d0eb-7301-b580-7b77cdcfaafe";
const FORK_BEFORE: &str = "01a1de97-d0f4-7e36-a832-36e738963e34";

#[test]
fn the_polled_list_merges_with_the_threads_this_app_watches() -> Check {
    // Rows 0 to 28: one turn, then `session/list` and its answer.
    let (model, _) = played(&head("lists.jsonl", 29)?)?;
    assert_eq!(model.wire.threads.len(), 6);
    let listed = thread(&model, LISTED[0])?;
    assert_eq!(listed.title.as_deref(), Some("run the command"));
    assert_eq!(listed.cwd, "$PROJECT");
    assert_eq!(listed.state, Run::Idle { stop: None });
    assert_eq!(listed.last_activity, Millis(1_794_735_853_596));
    assert_eq!(listed.sync, Sync::Apart, "nothing of it was replayed");
    assert!(listed.transcript.messages.is_empty());

    let live = thread(&model, LIVE)?;
    assert_eq!(live.title.as_deref(), Some("something to fork"));
    assert_eq!(
        said(&live.transcript),
        ["user: something to fork", "assistant: faux: "],
        "a list must not disturb a thread this app watches"
    );
    assert_eq!(live.sync, Sync::Live(Some(10)));
    let ended = Run::Idle {
        stop: Some(AcpStopReason::EndTurn),
    };
    assert_eq!(live.state, ended, "the list knows less of how it stopped");

    let order: Vec<&str> = model.wire.listed().iter().map(|t| t.id.as_str()).collect();
    assert_eq!(
        order,
        [LIVE, LISTED[0], LISTED[4], LISTED[2], LISTED[1], LISTED[3]],
        "newest activity first when nothing is pinned"
    );
    Ok(())
}

#[test]
fn a_pin_is_kept_and_puts_its_thread_first() -> Check {
    // Rows 29 and 30 pin the thread; the second list at row 34 shows the pin.
    let (model, _) = played(&head("lists.jsonl", 31)?)?;
    assert_eq!(
        thread(&model, LIVE)?.pinned,
        Some(Millis(1_794_735_853_802)),
        "the pin's answer was not kept"
    );
    let (model, _) = played(&head("lists.jsonl", 35)?)?;
    assert_eq!(
        thread(&model, LIVE)?.pinned,
        Some(Millis(1_794_735_853_802))
    );
    assert_eq!(
        model.wire.listed().first().map(|t| t.id.as_str()),
        Some(LIVE)
    );
    Ok(())
}

#[test]
fn a_fork_is_a_new_thread_that_knows_where_it_came_from() -> Check {
    let (model, _) = played(&recording("lists.jsonl")?)?;
    assert_eq!(model.wire.threads.len(), 8, "six listed and two forks");
    let at = thread(&model, FORK_AT)?;
    assert_eq!(at.forked_from, Some(ThreadId::new(LIVE)));
    assert_eq!(at.title.as_deref(), Some("something to fork"));
    assert_eq!(at.cwd, "$OTHER");
    assert_eq!(at.prefill, None);
    assert_eq!(
        at.workdir,
        Some(Workdir {
            cwd: "$HOME/.yi/lanes/50c3946b3ed3f5c1/2".to_owned(),
            lane: Some("other".to_owned())
        })
    );
    let before = thread(&model, FORK_BEFORE)?;
    assert_eq!(before.forked_from, Some(ThreadId::new(LIVE)));
    assert_eq!(
        before.prefill.as_deref(),
        Some("something to fork"),
        "the prompt the fork left out goes back in the composer"
    );
    Ok(())
}

fn setting<'a>(model: &'a Model, id: &str) -> Option<&'a SettingRow> {
    model
        .wire
        .settings
        .iter()
        .filter_map(Kept::known)
        .find(|row| row.id == id)
}

#[test]
fn yis_settings_are_read_and_a_write_shows_in_them() -> Check {
    // Rows 39 and 40 read the settings; rows 41 and 42 set one.
    let (model, _) = played(&head("lists.jsonl", 41)?)?;
    assert_eq!(model.wire.settings.len(), 67);
    let row = setting(&model, "catalog.enabled").ok_or("no such row")?;
    assert_eq!((&row.value, &row.default), (&Value::Null, &json!(true)));
    let (model, _) = played(&head("lists.jsonl", 43)?)?;
    let row = setting(&model, "catalog.enabled").ok_or("no such row")?;
    assert_eq!(row.value, json!(true));
    Ok(())
}

#[test]
fn what_the_server_refuses_is_kept_for_whoever_asked() -> Check {
    let (model, _) = played(&recording("lists.jsonl")?)?;
    let refused: Vec<(&str, i64)> = model
        .wire
        .failures
        .iter()
        .filter_map(|failure| match &failure.why {
            Refusal::Server { code, message } => Some((message.as_str(), *code)),
            Refusal::Unsent | Refusal::Lost | Refusal::TimedOut => None,
        })
        .collect();
    assert_eq!(
        refused,
        [
            ("no setting named no.such.key", -32602),
            ("unknown model faux/faux-1 (use provider/id)", -32602),
            ("unknown mode nope; use ask|auto|yolo", -32602),
            ("unknown method _yi/workdir", -32601),
            ("unknown method _yi/no_such_method", -32601)
        ]
    );
    let asked: Vec<&Asked> = model.wire.failures.iter().map(|f| &f.asked).collect();
    assert_eq!(asked.first(), Some(&&Asked::Settings));
    assert_eq!(asked.get(1), Some(&&Asked::Options(ThreadId::new(LIVE))));
    assert_eq!(
        model.wire.settings.len(),
        67,
        "a refused write changed the rows"
    );
    let mode = thread(&model, LIVE)?.option("mode");
    assert_eq!(mode.map(|o| o.current_value.as_str()), Some("ask"));
    Ok(())
}

fn poll(model: &mut Model, at: u64) -> Vec<Effect> {
    step(model, Msg::Wire(WireEvent::Poll { at: Millis(at) }))
}

fn lists(effects: &[Effect]) -> Result<usize, Box<dyn std::error::Error>> {
    Ok(sent(effects)?
        .iter()
        .filter(|frame| frame["method"] == "session/list")
        .count())
}

#[test]
fn a_poll_asks_for_the_list_once_and_gives_up_on_a_silent_server() -> Check {
    let mut model = Model::default();
    let up = WireEvent::Up {
        at: Millis(1_000),
        server: "0.1041.0".to_owned(),
        latency: None,
    };
    step(&mut model, Msg::Wire(up));
    let first = poll(&mut model, 1_000);
    assert_eq!(
        sent(&first)?,
        [json!({"jsonrpc": "2.0", "id": 0, "method": "session/list", "params": {}})]
    );
    assert_eq!(lists(&poll(&mut model, 6_000))?, 0, "one list at a time");
    // Thirty seconds without an answer: the request is given up and asked again.
    assert_eq!(lists(&poll(&mut model, 31_001))?, 1);
    let timed_out: Vec<&Refusal> = model.wire.failures.iter().map(|f| &f.why).collect();
    assert_eq!(timed_out, [&Refusal::TimedOut]);
    Ok(())
}

#[test]
fn a_thread_the_list_drops_goes_unless_this_app_holds_it() -> Check {
    let (mut model, _) = played(&head("lists.jsonl", 29)?)?;
    assert!(matches!(model.wire.link, Link::Up { .. }));
    let effects = poll(&mut model, 1);
    let id = sent(&effects)?
        .first()
        .map(|frame| frame["id"].clone())
        .ok_or("the poll sent nothing")?;
    // The server's list now holds one session: the rest were deleted.
    let only = json!({"sessionId": LISTED[1], "cwd": "$PROJECT", "title": "warm up",
        "_meta": {"yi": {"unseen": 2, "lastState": "requires_action",
            "lastEventMs": 9, "pinnedMs": null}}});
    let reply = json!({"jsonrpc": "2.0", "id": id, "result": {"sessions": [only]}});
    step(&mut model, Msg::Wire(WireEvent::Line(reply.to_string())));
    let left: Vec<&str> = model.wire.threads.keys().map(ThreadId::as_str).collect();
    assert_eq!(left, [LISTED[1], LIVE], "the watched thread stays");
    let kept = thread(&model, LISTED[1])?;
    assert_eq!((kept.unseen(), &kept.state), (2, &Run::NeedsYou));
    Ok(())
}

#[test]
fn what_a_server_can_make_this_app_keep_is_bounded() -> Check {
    let (mut model, _) = played(&recording("turn.jsonl")?)?;
    let id = "01a1de97-bf6c-7514-9fae-b57db1eaa7e1";
    let frame = |fields: Value| super::update(id, fields);
    for n in 0..2_000_u64 {
        let notice = json!({"sessionUpdate": "_yi/notice", "text": format!("notice {n}")});
        super::feed(&mut model, 1, &frame(notice));
        let kind = json!({"sessionUpdate": format!("_yi/kind_{n}"), "n": n});
        super::feed(&mut model, 1, &frame(kind));
        // The recorded turn ended at event 10; these two carry on from it.
        let event = json!({"sessionUpdate": "_yi/event", "seq": 11 + 2 * n,
            "event": {"type": format!("kind_{n}")}});
        super::feed(&mut model, 1, &frame(event));
        let child = json!({"sessionUpdate": "_yi/event", "seq": 12 + 2 * n,
            "childId": format!("a{n}"), "event": {"type": "agent_start"}});
        super::feed(&mut model, 1, &frame(child));
        // A call nobody made answered with a refusal: each is one failure to keep.
        let refused = json!({"jsonrpc": "2.0", "id": 5_000 + n, "error": {"code": 1}});
        let asked = json!({"jsonrpc": "2.0", "id": 5_000 + n, "method": "_yi/x", "params": {}});
        model.wire.sent(&asked, Millis(1));
        step(&mut model, Msg::Wire(WireEvent::Line(refused.to_string())));
    }
    let thread = thread(&model, id)?;
    assert_eq!(thread.notes.len(), MAX_NOTES);
    assert_eq!(
        thread.notes.last().map(|note| note.text.as_str()),
        Some("notice 1999"),
        "the newest note is the one kept"
    );
    assert_eq!(thread.extras.len(), MAX_EXTRAS);
    assert_eq!(thread.transcript.unread.len(), MAX_UNREAD);
    assert_eq!(thread.agents.len(), MAX_AGENTS);
    assert_eq!(model.wire.failures.len(), MAX_FAILURES);
    Ok(())
}

#[test]
fn a_listed_child_made_elsewhere_nests_under_its_parent() -> Check {
    let (mut model, _) = played(&head("lists.jsonl", 29)?)?;
    // Yi's list rows name no parent yet: this is the stored header's own field name.
    let row = |id: &str, parent: Value| json!({"sessionId": id, "cwd": "$PROJECT", "_meta": {"yi": parent}});
    let list = json!({"sessions": [
        row(LISTED[0], json!({})),
        row("child-made-elsewhere", json!({"parentSessionId": LISTED[0]})),
        row("its-own-parent", json!({"parentSessionId": "its-own-parent"})),
    ]});
    model.wire.merge(&list, None);
    let parent_of = |id: &str| thread(&model, id).map(|thread| thread.forked_from.clone());
    assert_eq!(
        parent_of("child-made-elsewhere")?,
        Some(ThreadId::new(LISTED[0]))
    );
    assert_eq!(
        parent_of("its-own-parent")?,
        None,
        "a thread is not its own child"
    );
    assert_eq!(parent_of(LISTED[0])?, None);
    Ok(())
}

#[test]
fn the_link_keeps_how_long_the_server_took_to_answer() -> Check {
    let mut model = Model::default();
    let up = WireEvent::Up {
        at: Millis(1_000),
        server: "0.1041.0".to_owned(),
        latency: Some(Millis(12)),
    };
    step(&mut model, Msg::Wire(up));
    let Link::Up { latency, .. } = model.wire.link else {
        return Err("the link is not up".into());
    };
    assert_eq!(latency, Some(Millis(12)));
    Ok(())
}

/// Answers every thread list in `asked` as the recorded server answered the same params.
fn answer_lists(model: &mut Model, asked: &[Effect], answers: &[(Value, Value)]) -> Check {
    for frame in sent(asked)? {
        let Some((_, result)) = answers
            .iter()
            .find(|(params, _)| *params == frame["params"])
        else {
            continue;
        };
        let reply = json!({"jsonrpc": "2.0", "id": frame["id"], "result": result});
        step(model, Msg::Wire(WireEvent::Line(reply.to_string())));
    }
    Ok(())
}

#[test]
fn a_thread_stored_in_a_known_folder_stays_in_the_sidebar_after_a_reconnect() -> Check {
    use crate::settings::schema::SettingValue;
    use crate::settings::store::{PROJECTS, PROJECTS_ROW, SettingsMsg};
    // A thread `yi ask` made in $PROJECT: the server's live list is empty, the folder's is not.
    let rows = super::rows("stored-list.jsonl")?;
    let asked = rows
        .iter()
        .filter(|(recv, frame)| !recv && frame["method"] == "session/list");
    let answers: Vec<(Value, Value)> = asked
        .filter_map(|(_, frame)| {
            let reply = rows
                .iter()
                .find(|(recv, r)| *recv && r["id"] == frame["id"])?;
            Some((frame["params"].clone(), reply.1["result"].clone()))
        })
        .collect();
    let stored = "01a1e351-7d1c-75e4-86e8-ff8029d4ff7e";
    let (mut model, _) = played(&head("stored-list.jsonl", 2)?)?;
    let declare = SettingsMsg::Declare(vec![PROJECTS_ROW]);
    step(&mut model, Msg::Settings(declare));
    let known = SettingValue::Json(r#"["$PROJECT"]"#.into());
    let set = SettingsMsg::Set(crate::ids::SettingKey::new(PROJECTS), Some(known));
    step(&mut model, Msg::Settings(set));
    for round in 0..2 {
        if round == 1 {
            let why = crate::wire::event::Down::Closed("the server restarted".to_owned());
            step(
                &mut model,
                Msg::Wire(WireEvent::Down { at: Millis(1), why }),
            );
            let server = "0.1041.0".to_owned();
            let up = WireEvent::Up {
                at: Millis(2),
                server,
                latency: None,
            };
            step(&mut model, Msg::Wire(up));
        }
        let poll = step(&mut model, Msg::Wire(WireEvent::Poll { at: Millis(3) }));
        let lists: Vec<Value> = sent(&poll)?
            .into_iter()
            .filter(|frame| frame["method"] == "session/list")
            .map(|frame| frame["params"].clone())
            .collect();
        assert_eq!(
            lists,
            [json!({"cwd": "$PROJECT"}), json!({})],
            "one poll, two lists"
        );
        answer_lists(&mut model, &poll, &answers)?;
        let shown: Vec<&str> = model.wire.listed().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            shown,
            [stored],
            "round {round}: the stored thread is listed"
        );
        assert!(
            model.wire.listed,
            "the list is known once every list of the round came"
        );
    }
    Ok(())
}

#[test]
fn watching_a_thread_does_not_move_it_above_one_the_server_says_moved_later() -> Check {
    let (mut model, _) = played(&head("lists.jsonl", 29)?)?;
    let later = thread(&model, LIVE)?.last_activity.0.saturating_add(1_000);
    let asked = poll(&mut model, later);
    let row = json!({"sessionId": LISTED[3], "cwd": "$PROJECT",
        "_meta": {"yi": {"lastState": "idle", "lastEventMs": later}}});
    answer_lists(
        &mut model,
        &asked,
        &[(json!({}), json!({"sessions": [row]}))],
    )?;
    let first = |model: &Model| {
        model
            .wire
            .listed()
            .first()
            .map(|t| t.id.as_str().to_owned())
    };
    assert_eq!(
        first(&model).as_deref(),
        Some(LISTED[3]),
        "the newest change"
    );
    step(&mut model, Msg::Tick(Millis(later.saturating_add(5_000))));
    let event = json!({"jsonrpc": "2.0", "method": "session/update", "params": {
        "sessionId": LIVE, "update": {"sessionUpdate": "_yi/event",
        "event": {"type": "turn_start"}, "seq": 11}}});
    step(&mut model, Msg::Wire(WireEvent::Line(event.to_string())));
    assert_eq!(
        first(&model).as_deref(),
        Some(LISTED[3]),
        "a step heard only because the thread is shown is no new change to the list"
    );
    Ok(())
}
