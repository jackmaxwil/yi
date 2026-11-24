//! The pane's scenes: frame 10's hub and frame 6's list from their fixtures, and a live thread
//! whose agent raised suggestions, one of them being accepted.

use yd_core::feed::{FeedEvent, FeedStatus};
use yd_core::findings::triage::{Act, TriageMsg};
use yd_core::findings::view::{Scope, findings};
use yd_core::ids::{FeedId, Millis, RowId, ThreadId};
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::reduce::turns::ToolCall;
use yd_core::row::{Row, Value, rows_from_json};
use yd_core::wire::replay::play_line;
use yd_ui::list::state::Lists;
use yd_ui::pane::{SCENE_PANE, Scene, Stage};
use yd_ui::win::Hub;

use super::{CHECKS, FEED, Form, View};

pub(super) const SCENES: &[Scene] = &[
    Scene {
        name: "findings/mockup",
        stage: Stage::Pane(PaneKind::Findings),
        setup: hub_mockup,
    },
    // The hub under the fixtures of its three side lists: parity reads a scene by its name.
    Scene {
        name: "findings-gate/mockup",
        stage: Stage::Pane(PaneKind::Findings),
        setup: hub_mockup,
    },
    Scene {
        name: "findings-place/mockup",
        stage: Stage::Pane(PaneKind::Findings),
        setup: hub_mockup,
    },
    Scene {
        name: "findings-checks/mockup",
        stage: Stage::Pane(PaneKind::Findings),
        setup: hub_mockup,
    },
    Scene {
        name: "findings-table/mockup",
        stage: Stage::Pane(PaneKind::Findings),
        setup: list_mockup,
    },
    Scene {
        name: "findings/accepting",
        stage: Stage::Pane(PaneKind::Findings),
        setup: accepting,
    },
];

const HUB_ROWS: &str = include_str!("../../../../../fixtures/mockup/findings.rows.json");
const LIST_ROWS: &str = include_str!("../../../../../fixtures/mockup/findings-table.rows.json");
const DETAIL: &str = include_str!("../../../../../fixtures/mockup/data/finding_detail.json");
const RULES: &str = include_str!("../../../../../fixtures/mockup/data/checks.json");
/// The row both frames draw open (`meta.json`).
pub(super) const OPEN: &str = "bucket-take-can-underflow-when-the-clock-steps-back";
pub(super) const THREAD: &str = "rate-limit";
/// The mockup's clock: 17:58 on its day.
const NOW: u64 = 1_794_679_080_000;

// Incident: this crate cannot name serde_json, so a fixture is read through a call's arguments.
fn json(text: &str) -> ToolCall {
    ToolCall {
        args: text.parse().unwrap_or_default(),
        ..ToolCall::default()
    }
}

fn say(hub: &mut Hub, line: &str) {
    // A scene line that does not decode is a scene bug the tests catch by what is missing.
    let _ = play_line(&mut hub.model, line);
}

fn recv(frame: &str) -> String {
    format!(r#"{{"dir":"recv","frame":{frame}}}"#)
}

/// The mockup's thread in `~/atlas-api`, made as a client sees it; a hub that has one keeps it.
pub(super) fn thread(hub: &mut Hub) {
    if !hub.model.wire.threads.is_empty() {
        return;
    }
    say(
        hub,
        &recv(
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":2,"info":{"name":"yi","version":"0.1041.0"}}}"#,
        ),
    );
    say(
        hub,
        r#"{"dir":"send","frame":{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":"~/atlas-api","mcpServers":[]}}}"#,
    );
    say(
        hub,
        &recv(&format!(
            r#"{{"jsonrpc":"2.0","id":2,"result":{{"sessionId":"{THREAD}"}}}}"#
        )),
    );
}

fn feed(hub: &mut Hub, id: FeedId, rows: Result<Vec<Row>, String>) {
    let event = match rows {
        Ok(rows) => FeedEvent::Reset(rows),
        Err(error) => FeedEvent::Status(FeedStatus::Error(error)),
    };
    step(&mut hub.model, Msg::Feed(id, event));
}

fn open(hub: &mut Hub, row: &str) {
    if let Some(lists) = hub.panes.get_mut::<Lists>(Some(SCENE_PANE)) {
        lists.list_mut("").set_open(Some(RowId::new(row)));
    }
}

fn form(hub: &mut Hub, form: Form) {
    if let Some(view) = hub.panes.get_mut::<View>(Some(SCENE_PANE)) {
        *view = View {
            form,
            project: form == Form::Hub,
            ..View::default()
        };
    }
}

/// The opened finding's detail, read from frame 10's own detail pane onto its row.
fn detailed(row: Row, page: &ToolCall) -> Row {
    let page = &page.args;
    let text = |name: &str| page.get(name).and_then(|v| v.as_str()).unwrap_or_default();
    let list = |name: &str| page.get(name).and_then(|v| v.as_array());
    let plain = |name: &str| {
        let all = list(name).into_iter().flatten();
        all.filter_map(|v| v.as_str().map(str::to_owned))
            .collect::<Vec<_>>()
    };
    let words = |name: &str, pick: &str| -> Vec<String> {
        let all = list(name).into_iter().flatten();
        all.filter_map(|item| item.get(pick)?.as_str().map(str::to_owned))
            .collect()
    };
    let labels = list("labels");
    let fix_note = labels
        .and_then(|labels| {
            labels
                .iter()
                .find(|l| l.get("label").and_then(|v| v.as_str()) == Some("Proposed fix"))
        })
        .and_then(|label| label.get("note")?.as_str())
        .unwrap_or_default();
    row.text("explanation", text("why"))
        .text("ref", text("id"))
        .text("fix_note", fix_note)
        .cell("fix", Value::Tags(words("fix", "text")))
        .cell("checks", Value::Tags(words("checks", "text")))
        .cell("check_notes", Value::Tags(words("checks", "note")))
        .cell("check_marks", Value::Tags(words("checks", "state")))
        .cell("history", Value::Tags(plain("history")))
        .cell("actions", Value::Tags(plain("buttons")))
}

/// Frame 10: the hub's rows and its open finding, with the standing checks beside them.
fn hub_mockup(hub: &mut Hub) {
    thread(hub);
    let page = json(DETAIL);
    let rows = rows_from_json(HUB_ROWS).map_err(|error| error.to_string());
    let rows = rows.map(|rows| {
        let fill = |row: Row| {
            if row.id.as_str() == OPEN {
                detailed(row, &page)
            } else {
                row
            }
        };
        rows.into_iter().map(fill).collect()
    });
    feed(hub, FEED, rows);
    let rules = json(RULES);
    let all = rules.args.as_array().into_iter().flatten().enumerate();
    let made = all.filter_map(|(n, rule)| {
        let on = rule.get("on").and_then(|v| v.as_bool()).unwrap_or(false);
        let row = Row::new(format!("rule-{n}"))
            .text("text", rule.get("text")?.as_str()?)
            .text("note", rule.get("note")?.as_str()?)
            .cell("on", Value::Flag(on));
        Some(row)
    });
    feed(hub, CHECKS, Ok(made.collect()));
    form(hub, Form::Hub);
    open(hub, OPEN);
}

/// Frame 6: the compact list of the shown thread, its underflow finding open.
fn list_mockup(hub: &mut Hub) {
    thread(hub);
    feed(
        hub,
        FEED,
        rows_from_json(LIST_ROWS).map_err(|error| error.to_string()),
    );
    form(hub, Form::List);
    open(hub, OPEN);
}

/// The thread's goal as the server sends it, its discoveries titled as frame 10's rows.
pub(super) fn goal(titles: &[(&str, bool)]) -> String {
    let found: Vec<String> = titles
        .iter()
        .enumerate()
        .map(|(n, (title, high))| {
            let violates = if *high {
                r#","violatesCheckOf":"t1""#
            } else {
                ""
            };
            format!(r#"{{"text":"{title}","fingerprint":"d{n}"{violates}}}"#)
        })
        .collect();
    let goal = format!(
        r#"{{"objective":"Uploads cannot be hammered","status":"active","tokensUsed":0,"timeUsedSeconds":0,"created":1,"updated":2,"check":"cargo test -p api upload::","discoveries":[{}]}}"#,
        found.join(",")
    );
    recv(&format!(
        r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"{THREAD}","update":{{"sessionUpdate":"_yi/goal","goal":{goal}}}}}}}"#
    ))
}

/// Two suggestions from the agent's goal: the High one is accepted and now blocks the gate,
/// the other still waits in triage.
fn accepting(hub: &mut Hub) {
    thread(hub);
    step(&mut hub.model, Msg::Tick(Millis(NOW)));
    let rows = rows_from_json(HUB_ROWS).unwrap_or_default();
    let title = |id: &str| {
        let row = rows.iter().find(|row| row.id.as_str() == id);
        row.and_then(|row| row.words("finding"))
            .unwrap_or(id)
            .to_owned()
    };
    let high = title(OPEN);
    let med = title("limit-is-keyed-by-user-id-only-anonymous-uploads-are-unlimited");
    say(hub, &goal(&[(&high, true), (&med, false)]));
    let shown = ThreadId::new(THREAD);
    let raised = findings(&hub.model, &shown, Scope::Project);
    if let Some(first) = raised.first() {
        let act = TriageMsg::Act {
            thread: shown,
            finding: first.id.clone(),
            act: Act::Accept,
        };
        step(&mut hub.model, Msg::Triage(act));
        open(hub, first.id.as_str());
    }
    form(hub, Form::Hub);
}
