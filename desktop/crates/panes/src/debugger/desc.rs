//! The debugger's side lists: variables, watches, call stack, breakpoints.

use yd_core::address::attach::ATTACH;
use yd_core::address::uri::Address;
use yd_core::command::table::{Command, OPEN_FILE, Route};
use yd_core::command::tools::debug::{BREAK, STEP_OUT};
use yd_core::feed::{FeedEvent, FeedStatus, Table};
use yd_core::ids::CommandId;
use yd_core::kind::Kind;
use yd_core::list::{Col, Detail, ListDesc, Main, RowHeight, Text, Width};
use yd_core::row::{Row, Rows, Tone, Value};

/// Buttons of an opened row that nothing carries out yet: pressing one says so.
pub const WATCH_VARIABLE: CommandId = CommandId("debugger.watch-variable");
pub const STOP_WHEN_CHANGES: CommandId = CommandId("debugger.stop-when-changes");
pub const STOP_WHEN_SET: CommandId = CommandId("debugger.stop-when-set");
pub const EDIT_WATCH: CommandId = CommandId("debugger.edit-watch");
pub const REMOVE_WATCH: CommandId = CommandId("debugger.remove-watch");
pub const ADD_CONDITION: CommandId = CommandId("debugger.add-condition");
pub const EDIT_CONDITION: CommandId = CommandId("debugger.edit-condition");
pub const TURN_ON: CommandId = CommandId("debugger.turn-on");
pub const TURN_OFF: CommandId = CommandId("debugger.turn-off");

/// An opened row's button, declared so the row draws it; the pane carries Watch, and the rest
/// say they are not available yet until the debugger takes them.
pub const fn row_command(id: CommandId, title: &'static str, kinds: &'static [Kind]) -> Command {
    Command {
        id,
        title,
        key: None,
        menu: None,
        kinds,
        route: Route::Model,
    }
}

/// The names of the four lists, which key them and keep their open rows.
pub const VARIABLES_PART: &str = "variables";
pub const WATCH_PART: &str = "watch";
pub const STACK_PART: &str = "stack";
pub const BREAKPOINTS_PART: &str = "breakpoints";

/// The prefixes the source gives its rows.
pub const VARIABLE: &str = "var:";
pub const WATCH: &str = "watch:";
pub const FRAME: &str = "frame:";
pub const BREAKPOINT: &str = "bp:";

const fn col(field: &'static str, width: Width, text: Text) -> Col {
    Col {
        field,
        title: "",
        width,
        text,
    }
}

const fn list(
    kind: Kind,
    columns: &'static [Col],
    tree: bool,
    detail: fn(&Row) -> Detail,
) -> ListDesc {
    ListDesc {
        kind,
        columns,
        tree,
        height: RowHeight::Row,
        facts_per_line: 1,
        detail,
        footer: None,
    }
}

pub const VARIABLES: ListDesc = list(
    Kind::Variable,
    &[
        col("name", Width::Px(90.0), Text::Name),
        col("value", Width::Px(56.0), Text::Plain),
        col("shown_type", Width::Fr(1.0), Text::Faint),
    ],
    true,
    variable,
);

pub const WATCHES: ListDesc = list(
    Kind::Variable,
    &[
        col("expression", Width::Fr(1.0), Text::Name),
        col("value", Width::Px(70.0), Text::Plain),
    ],
    false,
    watch,
);

pub const STACK: ListDesc = list(
    Kind::StackFrame,
    &[
        col("frame", Width::Px(18.0), Text::Name),
        col("function", Width::Fr(1.0), Text::Plain),
        col("at", Width::Px(84.0), Text::Faint),
    ],
    false,
    frame,
);

pub const BREAKPOINTS: ListDesc = list(
    Kind::Breakpoint,
    &[
        col("mark", Width::Px(12.0), Text::Plain),
        col("where", Width::Px(96.0), Text::Name),
        col("condition", Width::Fr(1.0), Text::Faint),
        col("hits", Width::Px(40.0), Text::Faint),
    ],
    false,
    breakpoint,
);

/// The list a row of the `debugger` feed belongs to, by its id.
pub fn of_row(id: &str) -> Option<&'static ListDesc> {
    [
        (VARIABLE, &VARIABLES),
        (WATCH, &WATCHES),
        (FRAME, &STACK),
        (BREAKPOINT, &BREAKPOINTS),
    ]
    .into_iter()
    .find_map(|(prefix, desc)| id.starts_with(prefix).then_some(desc))
}

/// The rows of `table` that start with `prefix`, each drawn as `shape` makes it.
pub fn shown(table: &Table, prefix: &str, shape: fn(Row) -> Row) -> Table {
    let part = table.prefixed(prefix);
    let rows = (0..part.len()).filter_map(|at| part.get(at)).cloned();
    let mut drawn = Table::default();
    drawn.apply(FeedEvent::Reset(rows.map(shape).collect()));
    drawn.apply(FeedEvent::Status(FeedStatus::Live));
    drawn
}

fn flag(row: &Row, field: &str) -> bool {
    row.get(field) == Some(&Value::Flag(true))
}

/// A changed value is drawn in the need colour, and its type says what it was.
pub fn variable_row(row: Row) -> Row {
    let kind = row.words("type").unwrap_or_default().to_owned();
    let shown = match row.words("was") {
        Some(was) if kind.is_empty() => format!("was {was}"),
        Some(was) => format!("{kind} · was {was}"),
        None => kind,
    };
    let changed = flag(&row, "changed");
    let row = row.text("shown_type", shown);
    Row {
        tone: changed.then_some(Tone::Need),
        ..row
    }
}

pub fn watch_row(row: Row) -> Row {
    let changed = flag(&row, "changed");
    Row {
        tone: changed.then_some(Tone::Need),
        ..row
    }
}

/// The current frame's words are bright, the others dim.
pub fn frame_row(row: Row) -> Row {
    let current = flag(&row, "current");
    Row {
        tone: current.then_some(Tone::Fg),
        ..row
    }
}

/// An enabled breakpoint is a dot; a disabled one an empty ring, at half strength.
pub fn breakpoint_row(row: Row) -> Row {
    let on = flag(&row, "enabled");
    Row {
        dim: !on,
        ..row.text("mark", if on { "●" } else { "○" })
    }
}

fn fact(row: &Row, label: &str, field: &str) -> Option<(String, Value)> {
    let said = row.words(field).filter(|said| !said.is_empty())?;
    Some((label.to_owned(), Value::Text(said.to_owned())))
}

fn was(row: &Row) -> Option<(String, Value)> {
    let said = row.words("was").filter(|said| !said.is_empty())?;
    Some((
        "Was".to_owned(),
        Value::Text(format!("{said}, one stop ago")),
    ))
}

/// The line a row names with its `path` and `line` cells.
pub fn line_of(row: &Row) -> Option<Address> {
    let path = row.words("path").filter(|path| !path.is_empty())?;
    let line: u32 = row.words("line")?.parse().ok()?;
    Some(
        Address::new(Kind::Lines, path)
            .ok()?
            .part(format!("L{line}")),
    )
}

fn detail_of(
    facts: impl IntoIterator<Item = Option<(String, Value)>>,
    buttons: Vec<Button>,
) -> Detail {
    let mut detail = Detail {
        facts: facts.into_iter().flatten().collect(),
        main: Main::Neither,
        ..Detail::default()
    };
    for (id, title, aim) in buttons {
        detail.commands.push(id);
        detail.titles.push((id, title.to_owned()));
        detail.aims.extend(aim.map(|aim| (id, aim)));
    }
    detail
}

/// A button: its command, its words, and the thing it is aimed at when not the row.
type Button = (CommandId, &'static str, Option<Address>);

const ATTACHED: Button = (ATTACH, "Attach", None);

fn variable(row: &Row) -> Detail {
    let stop = if row.words("state").is_some() {
        Some((STOP_WHEN_SET, "Stop when it is set", None))
    } else {
        flag(row, "changed").then_some((STOP_WHEN_CHANGES, "Stop when it changes", None))
    };
    let watching = row.words("state").is_none();
    let watch = watching.then_some((WATCH_VARIABLE, "Watch", None));
    let facts = [
        fact(row, "Type", "type"),
        fact(row, "Holds", "holds"),
        fact(row, "Lives at", "lives_at"),
        fact(row, "Now", "value"),
        was(row),
        fact(row, "Changed at", "changed_at"),
        fact(row, "Which is", "which_is"),
        fact(row, "Passed in by", "passed_in_by"),
        fact(row, "State", "state"),
    ];
    detail_of(
        facts,
        stop.into_iter().chain(watch).chain([ATTACHED]).collect(),
    )
}

fn watch(row: &Row) -> Detail {
    let first = match line_of(row) {
        Some(line) => (OPEN_FILE, "Open line", Some(line)),
        None => (EDIT_WATCH, "Edit", None),
    };
    let facts = [
        fact(row, "Result", "value"),
        fact(row, "Type", "type"),
        was(row),
        fact(row, "Set at", "set_at"),
    ];
    let buttons = vec![first, (REMOVE_WATCH, "Remove", None), ATTACHED];
    detail_of(facts, buttons)
}

fn frame(row: &Row) -> Detail {
    let facts = [
        fact(row, "Function", "function"),
        fact(row, "At", "at"),
        fact(row, "Variables", "variables"),
        fact(row, "Called with", "called_with"),
        fact(row, "From", "from"),
    ];
    let first = match line_of(row) {
        Some(line) => (OPEN_FILE, "Open line", Some(line)),
        None => (STEP_OUT, "Step out to your code", None),
    };
    detail_of(facts, vec![first, ATTACHED])
}

fn breakpoint(row: &Row) -> Detail {
    let on = flag(row, "enabled");
    let condition = row.words("condition").filter(|said| !said.is_empty());
    let when = row
        .words("stops_when")
        .or(condition.map(|said| said.trim_start_matches("if ")));
    let stops = row
        .words("stops")
        .or((on && when.is_none()).then_some("every time"));
    let said = |label: &str, said: Option<&str>| {
        said.map(|said| (label.to_owned(), Value::Text(said.to_owned())))
    };
    let facts = [
        said("Stops", stops),
        said("Stops when", when),
        fact(row, "Hit", "hit"),
        fact(row, "Added", "added"),
        fact(row, "State", "state"),
    ];
    let edit = match (on, condition) {
        (false, _) => None,
        (true, Some(_)) => Some((EDIT_CONDITION, "Edit condition", None)),
        (true, None) => Some((ADD_CONDITION, "Add a condition", None)),
    };
    let power = if on {
        (TURN_OFF, "Turn off", None)
    } else {
        (TURN_ON, "Turn on", None)
    };
    let remove = line_of(row).map(|line| (BREAK, "Remove", Some(line)));
    let buttons = edit
        .into_iter()
        .chain([power])
        .chain(remove)
        .chain([ATTACHED]);
    detail_of(facts, buttons.collect())
}
