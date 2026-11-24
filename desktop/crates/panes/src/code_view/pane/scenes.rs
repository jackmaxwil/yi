//! The code viewer's scenes: the mockup's open file, every kind of mark at once, and a panel
//! open between two lines. The file, its tabs and the panel's rows come from the mockup.

use std::rc::Rc;

use rune::{El, TextStyle, col, row, spacer, text};
use yd_core::address::uri::Address;
use yd_core::command::run::opened;
use yd_core::command::table::{OPEN_FILE, Route, Run, Target};
use yd_core::feed::{FILES, FeedEvent, OPEN_FILES};
use yd_core::ids::{FeedId, PaneId};
use yd_core::kind::Kind;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Rows, Tone, rows_from_json};
use yd_ui::code::doc::{Cols, LineNo};
use yd_ui::code::marks::{Glyph, GutterMark, Marks, Tint};
use yd_ui::code::rows::line_of;
use yd_ui::code::view::Gutter;
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::pane::{SCENE_PANE, Scene, Stage};
use yd_ui::theme::look::{CODE_LINE, LABEL, colour};
use yd_ui::theme::scale::{SEMIBOLD, WELL};
use yd_ui::win::Hub;

use super::{Slot, Viewer, path};

/// The sample references of `take`, the rows a slot's panel lists.
const REFERENCES: FeedId = OPEN_FILES.instance(1);
/// Values a paused debugger shows after a line's end, by line.
const VALUES: FeedId = OPEN_FILES.instance(2);
/// The mockup's file, as its tab names it.
const FILE: &str = "src/middleware/rate_limit.rs";

const BOLD: TextStyle = TextStyle {
    weight: SEMIBOLD,
    ..CODE_LINE
};

const STAGE: Stage = Stage::Pane(PaneKind::Code);

pub(super) const SCENES: &[Scene] = &[
    Scene {
        name: "code/empty",
        stage: Stage::Pane(PaneKind::Code),
        setup: |_| {},
    },
    Scene {
        name: "code/mockup",
        stage: STAGE,
        setup: file,
    },
    Scene {
        name: "code/marks",
        stage: STAGE,
        setup: marks,
    },
    Scene {
        name: "code/slot-open",
        stage: STAGE,
        setup: slot_open,
    },
    super::super::refs::SCENES[0],
];

/// Folds a sample's rows into `feed` as its source would.
fn feed(hub: &mut Hub, feed: FeedId, rows: &str) {
    let rows = rows_from_json(rows).unwrap_or_default();
    step(&mut hub.model, Msg::Feed(feed, FeedEvent::Reset(rows)));
}

/// Two files open, the first read by the files source and new in this thread (record 4.3).
/// Each of the sample's tabs is opened with `code.open`, the one it shows last.
fn file(hub: &mut Hub) {
    // The files source puts a read file's row beside the tree's rows: so does the sample.
    for read in rows_from_json(include_str!("../sample/fs.rows.json")).unwrap_or_default() {
        step(&mut hub.model, Msg::Feed(FILES, FeedEvent::Upsert(read)));
    }
    let sample = rows_from_json(include_str!("../sample/code.rows.json")).unwrap_or_default();
    let shown = sample.iter().max_by_key(|tab| opened(tab));
    for tab in sample.iter().chain(shown) {
        if let Ok(file) = Address::new(Kind::File, path(tab)) {
            let run = Run::new(OPEN_FILE).on(Target::Thing(file));
            step(&mut hub.model, Msg::Command(Route::Model, run));
        }
    }
    // Nothing marks a file as new in its thread yet: the sample's own flag stands in.
    for tab in sample.iter().filter(|tab| tab.get("new").is_some()) {
        let open = hub.model.tables.get(OPEN_FILES);
        let kept = open.and_then(|open| open.find(&tab.id)).cloned();
        let new = tab.get("new").cloned();
        if let (Some(kept), Some(new)) = (kept, new) {
            let marked = FeedEvent::Upsert(kept.cell("new", new));
            step(&mut hub.model, Msg::Feed(OPEN_FILES, marked));
        }
    }
}

fn line(n: u32) -> LineNo {
    LineNo::try_from(n).unwrap_or(LineNo::from_index(0))
}

/// The names the record marks as ones to follow (6.3): `LIMIT`, `Bucket` and `take`.
fn names(marks: Marks, open: bool) -> Marks {
    let name = |marks: Marks, n: u32, a: u32, b: u32, open: bool| {
        marks.follow(line(n), Cols::new(a, b), open)
    };
    let marks = name(marks, 3, 6, 11, false);
    let marks = name(marks, 15, 26, 31, false);
    let marks = name(marks, 7, 11, 17, false);
    let marks = name(marks, 12, 5, 11, false);
    name(marks, 13, 11, 15, open)
}

/// Everything a debugger and a language server put on the file (records 6.2 and 6.4.2), and a
/// removed line and an error so every kind of mark shows.
fn marks(hub: &mut Hub) {
    file(hub);
    feed(hub, VALUES, include_str!("../sample/values.rows.json"));
    let values = hub.model.tables.get(VALUES);
    let values = values.into_iter().flat_map(|table| {
        let rows = (0..table.len()).filter_map(|at| table.get(at));
        rows.filter_map(|row| Some((line_of(row.id.as_str())?, row.words("value")?.to_owned())))
    });
    let marked = values.fold(Marks::new(), |marks, (line, value)| {
        marks.value(line, value)
    });
    let marked = marked
        .gutter(line(14), GutterMark::Breakpoint)
        .gutter(line(18), GutterMark::Breakpoint)
        .tint(line(18), Tint::Current)
        .tint(line(10), Tint::Removed)
        .hint(line(19), 21, ": u32")
        .squiggle(line(20), Cols::new(24, 44), Tone::Need)
        .squiggle(line(4), Cols::new(35, 44), Tone::Bad)
        .glyph(line(20), Glyph::Fix);
    if let Some(viewer) = hub.panes.get_mut::<Viewer>(Some(SCENE_PANE)) {
        viewer.marks = Some((FILE.to_owned(), Rc::new(names(marked, false))));
        viewer.gutter = Gutter {
            marks: true,
            signs: true,
        };
    }
}

/// The references of `take` open under the line that defines it (record 6.3).
fn slot_open(hub: &mut Hub) {
    file(hub);
    feed(hub, REFERENCES, include_str!("../sample/refs.rows.json"));
    if let Some(viewer) = hub.panes.get_mut::<Viewer>(Some(SCENE_PANE)) {
        viewer.marks = Some((FILE.to_owned(), Rc::new(names(Marks::new(), true))));
        viewer.slot = Some(Slot {
            after: line(13),
            name: "references",
            build: references,
        });
    }
}

/// A stand-in for the references panel, which belongs to another task: a header, then each
/// file and its lines.
fn references(hub: &Hub, _: PaneId) -> El {
    let tokens = &hub.model.tokens;
    let (fg, dim, faint) = (colour(tokens.fg), colour(tokens.dim), colour(tokens.faint));
    let rows: Vec<&Row> = hub
        .model
        .tables
        .get(REFERENCES)
        .map_or(Vec::new(), |table| {
            (0..table.len()).filter_map(|at| table.get(at)).collect()
        });
    let mut files: Vec<&str> = rows.iter().filter_map(|row| row.group.as_deref()).collect();
    files.dedup();
    let said = |field: &str| rows.first().and_then(|row| row.words(field)).unwrap_or("");
    let count = format!(
        "{} · {} references in {} files",
        said("kind"),
        rows.len(),
        files.len()
    );
    let head = row()
        .w_full()
        .h(tokens.row)
        .px(tokens.gutter)
        .gap(7.0)
        .center_y()
        .fill(colour(tokens.context.alpha(0.12)))
        .child(text(said("name").to_owned()).font(BOLD).color(fg))
        .child(text(count).font(LABEL).color(dim))
        .child(spacer())
        .child(chip(tokens, Chip::Plain, "Attach all", None, false));
    let reference = |row: &&Row| {
        let number = text(row.words("line").unwrap_or_default().to_owned())
            .font(CODE_LINE)
            .color(faint);
        let code = text(row.words("text").unwrap_or_default().to_owned())
            .font(CODE_LINE)
            .color(dim);
        self::row()
            .w_full()
            .h(tokens.line)
            .pl(20.0)
            .pr(tokens.gutter)
            .gap(8.0)
            .child(self::row().w(22.0).justify_end().child(number))
            .child(code)
    };
    let group = |file: &str| {
        let here = rows.iter().filter(|row| row.group.as_deref() == Some(file));
        let name = text(file.to_owned()).font(CODE_LINE).color(faint);
        col()
            .w_full()
            .child(row().pt(4.0).pb(1.0).px(tokens.gutter).child(name))
            .children(here.map(reference))
    };
    col()
        .w_full()
        .pb(3.0)
        .fill(colour(WELL.of(tokens)))
        .border(1.0, colour(tokens.context.alpha(0.3)))
        .child(head)
        .children(files.into_iter().map(group))
}
