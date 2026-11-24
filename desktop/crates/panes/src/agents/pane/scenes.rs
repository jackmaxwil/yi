//! The pane's scenes: the mockup's rows from their fixture, a live thread built from wire
//! events, and the pane with nothing to show.

use yd_core::feed::{FeedEvent, FeedStatus};
use yd_core::ids::{Millis, RowId};
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::rows_from_json;
use yd_core::wire::replay::play_line;
use yd_ui::list::state::Lists;
use yd_ui::pane::{SCENE_PANE, Scene, Stage};
use yd_ui::win::Hub;

use super::{FEED, Limits};

pub(super) const SCENES: &[Scene] = &[
    Scene {
        name: "agents/mockup",
        stage: Stage::Pane(PaneKind::Agents),
        setup: mockup,
    },
    Scene {
        name: "agents/live",
        stage: Stage::Pane(PaneKind::Agents),
        setup: live,
    },
    Scene {
        name: "agents/working",
        stage: Stage::Pane(PaneKind::Agents),
        setup: working,
    },
];

/// A real server's turn whose `bash` call asks to run and is allowed.
const ALLOWED: &str = include_str!("../../../../../fixtures/wire/approval-allow.jsonl");
const ROWS: &str = include_str!("../../../../../fixtures/mockup/agents.rows.json");
const THREAD: &str = "rate-limit";
/// The mockup's clock: 17:58 on its day.
const NOW: u64 = 1_794_679_080_000;
const MINUTE: u64 = 60_000;

fn recv(frame: &str) -> String {
    format!(r#"{{"dir":"recv","frame":{frame}}}"#)
}

pub(super) fn update(body: &str) -> String {
    recv(&format!(
        r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"{THREAD}","update":{{{body}}}}}}}"#
    ))
}

fn at(hub: &mut Hub, millis: u64) {
    step(&mut hub.model, Msg::Tick(Millis(millis)));
}

pub(super) fn say(hub: &mut Hub, line: &str) {
    // A scene line that does not decode is a scene bug the tests catch by what is missing.
    let _ = play_line(&mut hub.model, line);
}

const OPTIONS: &str = r#"[{"configId":"model","name":"Model","type":"select","currentValue":"anthropic/claude-opus-5-5","options":[{"value":"anthropic/claude-opus-5-5","name":"Opus 5.5"}]}]"#;

/// The mockup's thread, made and named as a client sees it; a window's scene that already
/// shows one keeps it.
fn thread(hub: &mut Hub) {
    if !hub.model.wire.threads.is_empty() {
        return;
    }
    let new = r#"{"dir":"send","frame":{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"~/atlas-api","mcpServers":[]}}}"#;
    say(
        hub,
        &recv(
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2,"info":{"name":"yi","version":"0.1041.0"}}}"#,
        ),
    );
    say(hub, new);
    say(
        hub,
        &recv(&format!(
            r#"{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"{THREAD}","configOptions":{OPTIONS}}}}}"#
        )),
    );
    say(
        hub,
        &update(r#""sessionUpdate":"_yi/name","name":"Rate limit""#),
    );
}

/// Frame 6: the six agents of the record, from the fixture the way a feed would send them,
/// with the Tests agent open.
fn mockup(hub: &mut Hub) {
    thread(hub);
    let event = match rows_from_json(ROWS) {
        Ok(rows) => FeedEvent::Reset(rows),
        Err(error) => FeedEvent::Status(FeedStatus::Error(error.to_string())),
    };
    step(&mut hub.model, Msg::Feed(FEED, event));
    if let Some(lists) = hub.panes.get_mut::<Lists>(Some(SCENE_PANE)) {
        lists
            .list_mut("")
            .set_open(Some(RowId::new("tests-upload")));
    }
    if let Some(limits) = hub.panes.get_mut::<Limits>(Some(SCENE_PANE)) {
        *limits = Limits {
            cost: Some(2.0),
            slots: Some(8),
        };
    }
}

fn child(id: &str, name: &str, status: &str, activity: &str, extra: &str) -> String {
    format!(
        r#"{{"id":"{id}","name":"{name}","status":"{status}","activity":"{activity}","toolUseCount":1,"tokenCount":5000{extra}}}"#
    )
}

/// One agent's reply that calls `bash`, as the model writes it.
const BASH: &str = r#"{"role":"assistant","content":[{"type":"toolCall","id":"c5","name":"bash","arguments":{"command":"cargo test -p api upload::"}}],"api":"anthropic-messages","provider":"anthropic","model":"claude-opus-5-5","usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}},"stopReason":"stop","timestamp":0}"#;
const ASK: &str = r#"{"jsonrpc":"2.0","id":"perm-1","method":"session/request_permission","params":{"sessionId":"rate-limit","title":"bash requires permission","options":[{"optionId":"allow_once","name":"Allow once","kind":"allow_once"}]}}"#;

/// Events of the thread's stream, numbered as the server numbers them.
struct Stream<'a> {
    hub: &'a mut Hub,
    seq: u64,
}

impl Stream<'_> {
    /// An agent event on the thread's own stream, or on child `parent`'s.
    fn event(&mut self, parent: Option<&str>, event: &str) {
        let who = parent.map_or_else(String::new, |parent| format!(r#","childId":"{parent}""#));
        let seq = self.seq;
        self.seq = seq.saturating_add(1);
        let body = format!(r#""sessionUpdate":"_yi/event","seq":{seq}{who},"event":{event}"#);
        say(self.hub, &update(&body));
    }

    /// A child reported on the thread's own stream, or on another child's.
    fn report(&mut self, parent: Option<&str>, said: &str) {
        self.event(
            parent,
            &format!(r#"{{"type":"child_update","update":{said}}}"#),
        );
    }
}

/// A thread with agents three levels deep, as the server's events would build it: one done,
/// one waiting to run a command, one running under that one, one writing.
fn live(hub: &mut Hub) {
    thread(hub);
    let mut stream = Stream { hub, seq: 0 };
    at(stream.hub, NOW - 12 * MINUTE);
    stream.report(
        None,
        &child("fork", "Fork · sliding window", "running", "writing", ""),
    );
    at(stream.hub, NOW - 9 * MINUTE);
    let ended = r#","elapsedMs":180000,"answerPreview":"Sliding window kept as a branch.","session":"fork-session","exit":{"kind":"completed"}"#;
    stream.report(
        None,
        &child(
            "fork",
            "Fork · sliding window",
            "completed",
            "waiting",
            ended,
        ),
    );
    at(stream.hub, NOW - 6 * MINUTE);
    let task = r#","task":"Write tests for the new limiter and run them.""#;
    stream.report(
        None,
        &child("tests", "Tests · upload", "running", "waiting", task),
    );
    stream.event(
        Some("tests"),
        &format!(r#"{{"type":"message_end","message":{BASH}}}"#),
    );
    say(stream.hub, &recv(ASK));
    stream.event(None, r#"{"type":"permission_requested","toolCallId":"c5","title":"bash requires permission","description":"sandbox on"}"#);
    at(stream.hub, NOW - MINUTE);
    let bisect = child(
        "bisect",
        "Bisect the flaky case",
        "running",
        "executing",
        "",
    );
    stream.report(Some("tests"), &bisect);
    stream.report(
        None,
        &child("docs", "Docs · changelog", "running", "writing", ""),
    );
    say(
        stream.hub,
        &update(r#""sessionUpdate":"usage_update","used":41200,"size":200000"#),
    );
    say(
        stream.hub,
        &update(r#""sessionUpdate":"state_update","state":"requires_action""#),
    );
    at(stream.hub, NOW);
}

/// A turn with no child agent, waiting on the recorded ask to run its command.
fn working(hub: &mut Hub) {
    let upto: Vec<&str> = ALLOWED
        .lines()
        .take_while(|line| !line.contains("\"outcome\""))
        .collect();
    let _ = yd_core::wire::replay::play(&mut hub.model, &upto.join("\n"));
}
