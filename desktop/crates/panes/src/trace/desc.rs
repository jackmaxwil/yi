//! The Trace list: columns, the ruler and what an opened step shows.

use yd_core::command::table::{Command, Route};
use yd_core::ids::CommandId;
use yd_core::kind::Kind;
use yd_core::list::{Col, Detail, ListDesc, Main, RowHeight, Text, Width};
use yd_core::row::{Row, Tone, Value};

use crate::copy::labels;

/// Step, start and took, then the lane: the Step column is the one a nested step's indent
/// comes out of. The lane takes what is left, and the right gutter is the record's spacer.
pub const COLUMNS: &[Col] = &[
    column("step", Width::Px(150.0), Text::Name),
    column("start", Width::Px(34.0), Text::Number),
    column("took", Width::Px(44.0), Text::Number),
    column("timeline", Width::Fr(1.0), Text::Plain),
];

const fn column(field: &'static str, width: Width, text: Text) -> Col {
    Col {
        field,
        title: "",
        width,
        text,
    }
}

pub const DESC: ListDesc = ListDesc {
    kind: Kind::Span,
    columns: COLUMNS,
    tree: true,
    height: RowHeight::Row,
    facts_per_line: 3,
    detail,
    footer: None,
};

const fn does(id: &'static str, title: &'static str) -> Command {
    Command {
        id: CommandId(id),
        title,
        key: None,
        menu: None,
        kinds: &[Kind::Span],
        route: Route::Model,
    }
}

pub const RUN: CommandId = CommandId("trace.run");
pub const DENY: CommandId = CommandId("trace.deny");
pub const ATTACH: CommandId = CommandId("trace.attach");

/// The buttons of an opened step, in the record's words. The pane carries out run, deny
/// and attach (`super::pane::carry`); the rest say they are not available when pressed.
pub const COMMANDS: &[Command] = &[
    does("trace.logs", "Show its logs"),
    does("trace.changes", "Open the changes"),
    does("trace.wrote", "Show what it wrote"),
    does("trace.open-file", "Open the file"),
    does("trace.again", "Run it again"),
    does("trace.matches", "Show the matches"),
    does("trace.open-change", "Open the change"),
    does("trace.put-back", "Put it back"),
    does("trace.open-thread", "Open its thread"),
    does("trace.show-trace", "Show its trace"),
    does("trace.edit", "Edit"),
    does("trace.run", "Run it ↵"),
    does("trace.deny", "Deny"),
    does("trace.attach", labels::ATTACH),
];

/// A button is the command whose title is the words in the row's `actions` cell.
fn command(title: &str) -> Option<CommandId> {
    COMMANDS
        .iter()
        .find(|command| command.title == title)
        .map(|command| command.id)
}

type Pairs = &'static [(&'static str, &'static str)];

const TURN: Pairs = &[
    ("Asked", "asked"),
    ("Steps", "steps"),
    ("Agents", "agents"),
    ("Cost", "cost"),
    ("Tokens", "tokens"),
    ("Changed", "changed"),
];
const MODEL: Pairs = &[
    ("Model", "model"),
    ("Read", "read"),
    ("Wrote", "wrote"),
    ("First word", "first_word"),
    ("Speed", "speed"),
    ("Cost", "cost"),
    ("Decided", "decided"),
    ("Made", "made"),
];
const TOOL: Pairs = &[
    ("Tool", "tool"),
    ("Command", "command"),
    ("File", "file"),
    ("Looked for", "looked_for"),
    ("Got", "got"),
    ("Change", "change"),
];
const CHILD: Pairs = &[
    ("Agent", "agent"),
    ("Model", "model"),
    ("Steps", "steps"),
    ("Cost", "cost"),
    ("Wrote", "wrote"),
    ("State", "state"),
    ("Children", "children"),
];
const WAIT: Pairs = &[
    ("Command", "command"),
    ("Asked", "asked"),
    ("Waiting", "waiting"),
    ("Blocks", "blocks"),
    ("Runs in", "runs_in"),
    ("Takes", "takes"),
];

/// What kind of step a row is: its bar's colour says.
pub fn tone(row: &Row) -> Option<Tone> {
    match row.get("timeline")? {
        Value::Bar { tone, .. } => Some(*tone),
        _ => None,
    }
}

/// What the opened step read, wrote and cost, or what it touched and got, as its kind has it;
/// its buttons are the titles its `actions` cell holds.
fn detail(row: &Row) -> Detail {
    let tone = tone(row);
    let pairs = match tone {
        Some(Tone::Dim) => TURN,
        Some(Tone::Model) => MODEL,
        Some(Tone::Ok) => TOOL,
        Some(Tone::Accent) => CHILD,
        Some(Tone::Need) => WAIT,
        _ => &[],
    };
    let fact = |(label, field): &(&str, &str)| Some(((*label).to_owned(), row.get(field)?.clone()));
    let titles = match row.get("actions") {
        Some(Value::Tags(titles)) => titles.as_slice(),
        _ => &[],
    };
    Detail {
        facts: pairs.iter().filter_map(fact).collect(),
        commands: titles.iter().filter_map(|title| command(title)).collect(),
        main: if tone == Some(Tone::Need) {
            Main::First
        } else {
            Main::Neither
        },
        ..Detail::default()
    }
}
