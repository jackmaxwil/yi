//! The list engine's demo scene: four lists from the record's sample tables (5.3, 5.5, 5.6,
//! 5.8), a tree and three flat ones, each with one row open.

use crate::theme::look::{colour, white};
use rune::{Cx, El, col, row};
use yd_core::command::table::{Command, Route};
use yd_core::feed::{FeedEvent, FeedStatus};
use yd_core::ids::{CommandId, FeedId, PaneId, RowId};
use yd_core::kind::Kind;
use yd_core::list::{Col, Detail, ListDesc, Main, RowHeight, Text, Width};
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Value, rows_from_json};

use super::list;
use crate::kit::button::{Button, button};
use crate::list::state::Lists;
use crate::pane::{PaneCx, PaneDef, SCENE_PANE, Scene, Stage};
use crate::win::{Hub, Win};

pub(super) const SCENES: &[Scene] = &[
    Scene {
        name: "list/open-row",
        stage: Stage::View(view),
        setup,
    },
    Scene {
        name: "keys/focus",
        stage: Stage::View(focus),
        setup: picked,
    },
];

/// One list of the scene: its pane, its description, its sample rows and the row it opens.
struct Demo {
    pane: PaneId,
    kind: PaneKind,
    desc: ListDesc,
    rows: &'static str,
    open: &'static str,
}

const fn column(field: &'static str, title: &'static str, width: Width, text: Text) -> Col {
    Col {
        field,
        title,
        width,
        text,
    }
}

const fn does(id: &'static str, title: &'static str) -> Command {
    Command {
        id: CommandId(id),
        title,
        key: None,
        menu: None,
        kinds: &[],
        route: Route::Model,
    }
}

const AGENT_DOES: &[Command] = &[
    does("agent.run-command", "Run its command"),
    does("agent.open-thread", "Open thread"),
    does("agent.pause", "Pause"),
    does("agent.more-budget", "More budget"),
    does("agent.stop", "Stop"),
];
const LOG_DOES: &[Command] = &[
    does("log.show-in-trace", "Show in trace"),
    does("log.around", "Logs around it"),
    does("log.only-source", "Only docker:db"),
    does("log.attach", "Attach"),
    does("log.make-finding", "Make it a finding"),
];
const CONTAINER_DOES: &[Command] = &[
    does("container.logs", "Logs"),
    does("container.shell", "Shell"),
    does("container.restart", "Restart"),
    does("container.stop", "Stop"),
    does("container.attach", "Attach"),
];
const FINDING_DOES: &[Command] = &[
    does("finding.allow", "Allow the test"),
    does("finding.open-agent", "Open its agent"),
    does("finding.attach", "Attach"),
    does("finding.dismiss", "Dismiss"),
];

/// The scene draws its lists itself; these panes only declare the commands of its buttons.
fn undrawn(_: &PaneCx<'_>, _: &mut Cx<Win>) -> El {
    col()
}

const fn pane(kind: PaneKind, commands: &'static [Command]) -> PaneDef {
    PaneDef {
        kind,
        view: undrawn,
        scenes: &[],
        commands,
        settings: &[],
        tabs: None,
        on_tab: None,
        finder: None,
    }
}

static PANES: &[&PaneDef] = &[
    &pane(PaneKind::Agents, AGENT_DOES),
    &pane(PaneKind::Logs, LOG_DOES),
    &pane(PaneKind::Containers, CONTAINER_DOES),
    &pane(PaneKind::Findings, FINDING_DOES),
];

/// The facts of an opened row: each label with the cell it reads, where the row has one.
fn facts(row: &Row, pairs: &[(&str, &str)]) -> Vec<(String, Value)> {
    let fact = |(label, field): &(&str, &str)| Some(((*label).to_owned(), row.get(field)?.clone()));
    pairs.iter().filter_map(fact).collect()
}

fn ids(does: &[Command]) -> Vec<CommandId> {
    does.iter().map(|command| command.id).collect()
}

fn agent(row: &Row) -> Detail {
    let pairs = [
        ("Asked to", "asked"),
        ("Said last", "said"),
        ("Touched", "touched"),
        ("Budget", "budget"),
    ];
    Detail {
        facts: facts(row, &pairs),
        commands: ids(AGENT_DOES),
        quiet: vec![CommandId("agent.stop")],
        ..Detail::default()
    }
}

fn log(row: &Row) -> Detail {
    let pairs = [
        ("Time", "time"),
        ("Level", "level"),
        ("Source", "source"),
        ("Container", "container"),
        ("Image", "image"),
        ("Host", "host"),
        ("Peer", "peer"),
        ("Pool", "pool"),
        ("Seen", "seen"),
    ];
    Detail {
        facts: facts(row, &pairs),
        text: row.words("raw").map(str::to_owned),
        commands: ids(LOG_DOES),
        main: Main::Neither,
        ..Detail::default()
    }
}

fn container(row: &Row) -> Detail {
    let pairs = [
        ("Image", "image"),
        ("Ports", "ports"),
        ("Health", "health"),
        ("Memory", "mem"),
        ("Restarts", "restarts"),
        ("Last line", "last"),
    ];
    Detail {
        facts: facts(row, &pairs),
        text: None,
        commands: ids(CONTAINER_DOES),
        note: row.words("note").map(str::to_owned),
        ..Detail::default()
    }
}

fn finding(row: &Row) -> Detail {
    let pairs = [("Why", "why"), ("Proof", "proof"), ("Evidence", "evidence")];
    Detail {
        facts: facts(row, &pairs),
        commands: ids(FINDING_DOES),
        quiet: vec![CommandId("finding.dismiss")],
        ..Detail::default()
    }
}

/// Top left, bottom left, top right, bottom right.
const DEMOS: [Demo; 4] = [
    Demo {
        pane: SCENE_PANE,
        kind: PaneKind::Agents,
        desc: ListDesc {
            kind: Kind::Agent,
            columns: &[
                column("state", "", Width::Px(7.0), Text::Plain),
                column("name", "Agent", Width::Px(131.0), Text::Name),
                column("doing", "Doing now", Width::Fr(1.0), Text::Plain),
                column("model", "", Width::Px(12.0), Text::Mono),
                column("tokens", "Tokens", Width::Px(38.0), Text::Number),
                column("cost", "Cost", Width::Px(36.0), Text::Number),
                column("time", "Time", Width::Px(30.0), Text::Number),
                column("timeline", "Last 14 min", Width::Px(60.0), Text::Plain),
            ],
            tree: true,
            height: RowHeight::Row,
            facts_per_line: 1,
            detail: agent,
            footer: None,
        },
        rows: include_str!("../demo/agents.rows.json"),
        open: "tests",
    },
    Demo {
        pane: PaneId(1),
        kind: PaneKind::Logs,
        desc: ListDesc {
            kind: Kind::LogLine,
            columns: &[
                column("time", "", Width::Px(80.0), Text::Faint),
                column("level", "", Width::Px(36.0), Text::Plain),
                column("source", "", Width::Px(64.0), Text::Faint),
                column("message", "", Width::Fr(1.0), Text::Plain),
            ],
            tree: false,
            height: RowHeight::Line,
            facts_per_line: 3,
            detail: log,
            footer: None,
        },
        rows: include_str!("../demo/logs.rows.json"),
        open: "l10",
    },
    Demo {
        pane: PaneId(2),
        kind: PaneKind::Containers,
        desc: ListDesc {
            kind: Kind::Container,
            columns: &[
                column("state", "", Width::Px(10.0), Text::Plain),
                column("name", "Name", Width::Px(88.0), Text::Name),
                column("image", "Image", Width::Fr(1.0), Text::Mono),
                column("cpu", "CPU", Width::Px(68.0), Text::Plain),
                column("mem", "Mem", Width::Px(50.0), Text::Number),
                column("ports", "Ports", Width::Px(60.0), Text::Mono),
                column("up", "Up", Width::Px(46.0), Text::Number),
            ],
            tree: false,
            height: RowHeight::Row,
            facts_per_line: 3,
            detail: container,
            footer: None,
        },
        rows: include_str!("../demo/containers.rows.json"),
        open: "atlas-api",
    },
    Demo {
        pane: PaneId(3),
        kind: PaneKind::Findings,
        desc: ListDesc {
            kind: Kind::Finding,
            columns: &[
                column("severity", "", Width::Px(32.0), Text::Plain),
                column("title", "Finding", Width::Fr(1.0), Text::Name),
                column("where", "Where", Width::Px(104.0), Text::Mono),
                column("state", "State", Width::Px(58.0), Text::Plain),
                column("who", "Who has it", Width::Px(86.0), Text::Plain),
                column("age", "Age", Width::Px(24.0), Text::Number),
            ],
            tree: false,
            height: RowHeight::Row,
            facts_per_line: 1,
            detail: finding,
            footer: Some("From review, tests, logs, the desktop and your own comments"),
        },
        rows: include_str!("../demo/findings.rows.json"),
        open: "underflow",
    },
];

/// Each sample goes through `step` as its feed would send it, then one row is opened.
fn setup(hub: &mut Hub) {
    hub.defs = PANES;
    for demo in &DEMOS {
        let event = match rows_from_json(demo.rows) {
            Ok(rows) => FeedEvent::Reset(rows),
            Err(error) => FeedEvent::Status(FeedStatus::Error(error.to_string())),
        };
        step(
            &mut hub.model,
            Msg::Feed(FeedId::new(demo.kind.slug()), event),
        );
        if let Some(lists) = hub.panes.get_mut::<Lists>(Some(demo.pane)) {
            lists.list_mut("").set_open(Some(RowId::new(demo.open)));
        }
    }
}

/// The agents list with its cursor on a row and nothing open, as the keyboard leaves it.
fn picked(hub: &mut Hub) {
    setup(hub);
    if let Some(lists) = hub.panes.get_mut::<Lists>(Some(SCENE_PANE)) {
        let state = lists.list_mut("");
        state.set_open(None);
        state.select(&RowId::new("tests"), 2);
    }
}

/// A list and a button, each drawn with the ring the keyboard puts on a control.
fn focus(_: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    cx.force_focus_rings();
    let demo = &DEMOS[0];
    let pane = PaneCx {
        hub,
        pane: demo.pane,
        focused: true,
        edits: None,
    };
    let table = hub.model.tables.get(FeedId::new(demo.kind.slug()));
    let tokens = &hub.model.tokens;
    let allow = button(tokens, Button::Primary, "Allow once", Some("↵"));
    col()
        .w_full()
        .h_full()
        .padding(16.0)
        .gap(16.0)
        .child(
            col()
                .w_full()
                .h(240.0)
                .child(list(&pane, demo.kind, &demo.desc, table)),
        )
        .child(row().child(allow.on_press(|_: &mut Win| {})))
}

fn view(_: &Win, hub: &Hub, _: &mut Cx<Win>) -> El {
    let hairline = colour(white(hub.model.tokens.lines));
    let quarter = |demo: &Demo| {
        let pane = PaneCx {
            hub,
            pane: demo.pane,
            focused: demo.pane == SCENE_PANE,
            edits: None,
        };
        let table = hub.model.tables.get(FeedId::new(demo.kind.slug()));
        col()
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .border_sides([0.0, 1.0, 1.0, 0.0], hairline)
            .child(list(&pane, demo.kind, &demo.desc, table))
    };
    let half = |demos: &[Demo]| {
        col()
            .grow(1.0)
            .min_w(0.0)
            .h_full()
            .children(demos.iter().map(quarter))
    };
    let (left, right) = DEMOS.split_at(2);
    row().w_full().h_full().child(half(left)).child(half(right))
}
