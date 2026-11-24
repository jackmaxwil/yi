//! The pane's scenes: the paused stop of the mockup's frame 8, and a pane with nothing started.
//! The mockup's lists are turned into the rows the debugger source sends, then folded as feed events.

use yd_core::feed::{DEBUGGER, FILES, FeedEvent, FeedStatus};
use yd_core::ids::RowId;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Value, rows_from_json};
use yd_ui::list::state::Lists;
use yd_ui::pane::{SCENE_PANE, Scene, Stage};
use yd_ui::win::Hub;

use crate::debugger::desc::VARIABLES_PART;

pub(super) const SCENES: &[Scene] = &[
    Scene {
        name: "debugger/mockup",
        stage: Stage::Pane(PaneKind::Debugger),
        setup: paused,
    },
    Scene {
        name: "debugger/not-started",
        stage: Stage::Pane(PaneKind::Debugger),
        setup: not_started,
    },
];

const VARIABLES: &str = include_str!("../../../../../fixtures/mockup/debugger-variables.rows.json");
const WATCHES: &str = include_str!("../../../../../fixtures/mockup/debugger-watch.rows.json");
const STACK: &str = include_str!("../../../../../fixtures/mockup/debugger-stack.rows.json");
const BREAKPOINTS: &str =
    include_str!("../../../../../fixtures/mockup/debugger-breakpoints.rows.json");
/// The file the mockup's code viewer reads, as the files source would have read it.
const FILE_ROWS: &str = include_str!("../../code_view/sample/fs.rows.json");
/// The record's console (6.2), as the adapter's `out:` rows.
const CONSOLE: &str = include_str!("sample/console.rows.json");

const FILE: &str = "src/middleware/rate_limit.rs";
/// Where the mockup's other file lives; its stack and breakpoints name files by their short name.
const FILES_BY_NAME: [(&str, &str); 2] = [
    ("rate_limit.rs", FILE),
    ("upload.rs", "src/routes/upload.rs"),
];
/// The variables the mockup draws a disclosure arrow on.
const EXPANDABLE: [&str; 3] = ["self", "refill_at", "now"];

fn fixture(text: &str) -> Vec<Row> {
    rows_from_json(text).unwrap_or_default()
}

fn text(row: &Row, field: &str) -> String {
    row.words(field).unwrap_or_default().to_owned()
}

/// The row the source sends for a `file.rs:18` location: its `path` and `line` cells.
fn located(row: Row, at: &str) -> Row {
    let Some((name, line)) = at.rsplit_once(':') else {
        return row;
    };
    let path = FILES_BY_NAME.iter().find(|(short, _)| *short == name);
    match path {
        Some((_, path)) => row.text("path", *path).text("line", line),
        None => row,
    }
}

fn without_buttons(mut row: Row) -> Row {
    row.cells.remove("actions");
    row
}

fn variables() -> Vec<Row> {
    let rows = fixture(VARIABLES);
    let path = |row: &Row| {
        let parent = row
            .parent
            .as_ref()
            .and_then(|id| rows.iter().find(|r| r.id == *id));
        match parent {
            Some(parent) => format!("{}.{}", text(parent, "name"), text(row, "name")),
            None => text(row, "name"),
        }
    };
    rows.iter()
        .map(|row| {
            // The mockup writes a changed variable's type as `u32 · was 1`; the source sends both.
            let kind = text(row, "type");
            let (kind, was) = kind.split_once(" · was ").unwrap_or((&kind, ""));
            let mut sent = without_buttons(row.clone()).text("type", kind);
            sent.cells.remove("was");
            if !was.is_empty() {
                sent = sent.text("was", was);
            }
            let expandable = EXPANDABLE.contains(&text(row, "name").as_str());
            Row {
                id: RowId::new(format!("var:{}", path(row))),
                parent: row.parent.as_ref().map(|id| {
                    let parent = rows.iter().find(|r| r.id == *id);
                    RowId::new(format!(
                        "var:{}",
                        parent.map(|p| text(p, "name")).unwrap_or_default()
                    ))
                }),
                ..sent.cell("expandable", Value::Flag(expandable))
            }
        })
        .collect()
}

fn watches() -> Vec<Row> {
    let rows = fixture(WATCHES);
    (1..)
        .zip(rows)
        .map(|(n, row)| {
            let was = text(&row, "was");
            let bare = was
                .strip_suffix(", one stop ago")
                .unwrap_or(&was)
                .to_owned();
            let set_at = text(&row, "set_at");
            let mut sent = without_buttons(row);
            sent.id = RowId::new(format!("watch:{n}"));
            sent.cells.remove("was");
            if !bare.is_empty() {
                sent = sent.text("was", bare);
            }
            located(sent, &set_at)
        })
        .collect()
}

fn frames() -> Vec<Row> {
    let rows = fixture(STACK)
        .into_iter()
        .filter(|row| !text(row, "function").is_empty());
    rows.map(|row| {
        let n = text(&row, "frame").trim_start_matches('#').to_owned();
        let at = text(&row, "at");
        let mut sent = without_buttons(row);
        sent.id = RowId::new(format!("frame:{n}"));
        located(sent, &at)
    })
    .collect()
}

fn breakpoints() -> Vec<Row> {
    let rows = fixture(BREAKPOINTS);
    (1..)
        .zip(rows)
        .map(|(n, row)| {
            let place = text(&row, "where");
            let mut sent = without_buttons(row);
            sent.id = RowId::new(format!("bp:{n}"));
            located(sent, &place)
        })
        .collect()
}

fn state(status: &str) -> Row {
    Row::new("state")
        .text("adapter", "LLDB")
        .text("program", "api")
        .text("build", "debug build")
        .text("pid", "48213")
        .text("threads", "5")
        .text("status", status)
        .text("reason", "breakpoint 2")
        .text("path", FILE)
        .text("line", "18")
}

fn feed(hub: &mut Hub, rows: Vec<Row>) {
    step(&mut hub.model, Msg::Feed(DEBUGGER, FeedEvent::Reset(rows)));
    step(
        &mut hub.model,
        Msg::Feed(DEBUGGER, FeedEvent::Status(FeedStatus::Live)),
    );
}

/// Frame 8's left pane: stopped at breakpoint 2 on line 18 with `tokens` open.
fn paused(hub: &mut Hub) {
    for read in fixture(FILE_ROWS) {
        step(&mut hub.model, Msg::Feed(FILES, FeedEvent::Upsert(read)));
    }
    let mut rows = vec![state("paused")];
    rows.extend(variables());
    rows.extend(watches());
    rows.extend(frames());
    rows.extend(breakpoints());
    rows.extend(fixture(CONSOLE));
    feed(hub, rows);
    if let Some(lists) = hub.panes.get_mut::<Lists>(Some(SCENE_PANE)) {
        lists
            .list_mut(VARIABLES_PART)
            .set_open(Some(RowId::new("var:self.tokens")));
    }
}

/// The source has said it is idle: nothing launched yet.
fn not_started(hub: &mut Hub) {
    let idle = Row::new("state").text("status", "idle");
    feed(hub, vec![idle]);
}
