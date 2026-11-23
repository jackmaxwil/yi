//! The pane's scenes: the mockup's two tabs, and the code viewer with the servers' marks.

use std::rc::Rc;

use rune::{Cx, El, TextStyle, col, row, text};
use yd_core::address::uri::Address;
use yd_core::command::run::opened;
use yd_core::command::table::{OPEN_FILE, Route, Run, Target};
use yd_core::feed::{FILES, FeedEvent, LSP, OPEN_FILES};
use yd_core::grid::layout::Size;
use yd_core::ids::RowId;
use yd_core::kind::Kind;
use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Rows, rows_from_json};
use yd_ui::chrome::tabs::{self, Bar, FILE};
use yd_ui::code::colour::load;
use yd_ui::code::doc::{Cols, Doc, Lang, LineNo};
use yd_ui::code::pointer::Token;
use yd_ui::code::view::CodeView;
use yd_ui::grid_view::drag::At;
use yd_ui::kit::bars::{footer, header};
use yd_ui::kit::icons::glyph;
use yd_ui::list::state::Lists;
use yd_ui::pane::{PaneCx, SCENE_PANE, Scene, Stage};
use yd_ui::theme::look::{CODE_LINE, LABEL, colour};
use yd_ui::theme::scale::SEMIBOLD;
use yd_ui::win::{Hub, Win};

use super::{Show, State};
use crate::lsp::marks::{self, File, Kept, bar, bar_line, card};

const SERVERS: &str = include_str!("../../../../../fixtures/mockup/lsp.rows.json");
const PROBLEMS: &str = include_str!("../../../../../fixtures/mockup/problems.rows.json");
const FILE_ROWS: &str = include_str!("../../code_view/sample/fs.rows.json");
const TAB_ROWS: &str = include_str!("../../code_view/sample/code.rows.json");
/// The mockup's open file, and the server rows that describe it.
const PATH: &str = "src/middleware/rate_limit.rs";
const REFERENCES: usize = 12;
/// A hover as rust-analyzer words it: where the item is from, its signature, then its prose.
const HOVER: &str = "```rust\ncore::time\n```\n\n```rust\npub const fn from_secs(secs: u64) -> Duration\n```\n\nMakes a length of time from a whole number of seconds.";
const STAGE: Stage = Stage::Pane(PaneKind::LanguageServers);

pub(in crate::lsp) const SCENES: &[Scene] = &[
    Scene {
        name: "lsp/mockup",
        stage: STAGE,
        setup: servers,
    },
    Scene {
        name: "problems/mockup",
        stage: STAGE,
        setup: problems,
    },
    Scene {
        name: "lsp-code/mockup",
        stage: Stage::View(code),
        setup: marked,
    },
    // The same rows on the code viewer itself, as the running window draws them.
    Scene {
        name: "code/served",
        stage: Stage::Pane(PaneKind::Code),
        setup: marked,
    },
];

fn feed(hub: &mut Hub, rows: Vec<Row>) {
    step(&mut hub.model, Msg::Feed(LSP, FeedEvent::Reset(rows)));
}

fn fixture(json: &str) -> Vec<Row> {
    rows_from_json(json).unwrap_or_default()
}

/// The mockup's servers as the source names them.
pub(super) fn server_rows() -> Vec<Row> {
    fixture(SERVERS)
        .into_iter()
        .map(|mut row| {
            row.id = RowId::new(format!("server:{}", row.id));
            row
        })
        .collect()
}

/// The mockup's problems as the source writes them: the path and line its `where` cell names.
pub(super) fn problem_rows() -> Vec<Row> {
    let make = |row: Row| {
        let place = row.words("where").map(str::to_owned).unwrap_or_default();
        let (path, line) = place.rsplit_once(':').unwrap_or((place.as_str(), "1"));
        let (path, line) = (path.to_owned(), line.to_owned());
        let mut row = row.text("path", path.as_str()).text("line", line.as_str());
        row.id = RowId::new(format!("problem:{path}:{line}:0"));
        row
    };
    fixture(PROBLEMS).into_iter().map(make).collect()
}

fn open(hub: &mut Hub, list: &'static str, row: &str) {
    if let Some(lists) = hub.panes.get_mut::<Lists>(Some(SCENE_PANE)) {
        lists.list_mut(list).set_open(Some(RowId::new(row)));
    }
}

/// Frame 9's left pane: the servers with the first open, and the problems with the third.
fn servers(hub: &mut Hub) {
    feed(hub, [server_rows(), problem_rows()].concat());
    open(hub, "servers", "server:rust-analyzer");
    open(hub, "problems", "problem:rate_limit.rs:20:0");
}

/// The Problems tab in front, its open problem the mockup's.
fn problems(hub: &mut Hub) {
    servers(hub);
    if let Some(state) = hub.panes.get_mut::<State>(Some(SCENE_PANE)) {
        state.show = Show::Problems;
    }
}

fn file_text(hub: &Hub) -> Option<String> {
    let table = hub.model.tables.get(FILES)?;
    let file = table.find(&RowId::new(format!("file:{PATH}")))?;
    file.words("text").map(str::to_owned)
}

/// The byte range of `needle` in a line, counted from the line's first byte.
fn find(doc: &Doc, line: u32, needle: &str) -> Option<(u32, u32)> {
    let text = doc.line(LineNo::try_from(line).ok()?)?;
    let start = text.find(needle)?;
    Some((
        u32::try_from(start).ok()?,
        u32::try_from(start + needle.len()).ok()?,
    ))
}

/// Both tabs of the code viewer opened as `code.open` opens them, the file read as the files
/// source reads it.
fn open_files(hub: &mut Hub) {
    for read in fixture(FILE_ROWS) {
        step(&mut hub.model, Msg::Feed(FILES, FeedEvent::Upsert(read)));
    }
    let sample = fixture(TAB_ROWS);
    let shown = sample.iter().max_by_key(|tab| opened(tab));
    for tab in sample.iter().chain(shown) {
        if let Some(Ok(file)) = tab.words("path").map(|path| Address::new(Kind::File, path)) {
            let run = Run::new(OPEN_FILE).on(Target::Thing(file));
            step(&mut hub.model, Msg::Command(Route::Model, run));
        }
    }
    // Nothing marks a file as new in its thread yet: the sample's own flag stands in.
    let new = sample.iter().find(|tab| tab.get("new").is_some());
    let kept = new.and_then(|tab| hub.model.tables.get(OPEN_FILES)?.find(&tab.id).cloned());
    if let (Some(kept), Some(flag)) = (kept, new.and_then(|tab| tab.get("new").cloned())) {
        let event = FeedEvent::Upsert(kept.cell("new", flag));
        step(&mut hub.model, Msg::Feed(OPEN_FILES, event));
    }
}

/// A character column counted from 1, for byte `at` of a line of `doc`.
fn char_col(doc: &Doc, line: u32, at: u32) -> usize {
    let text = LineNo::try_from(line).ok().and_then(|line| doc.line(line));
    let before = text.and_then(|text| text.get(..usize::try_from(at).ok()?));
    before.map_or(0, |before| before.chars().count()) + 1
}

/// What the server told of the file: the warning on line 20 with a fix for it, a type hint
/// on line 19, and what a rest on `from_secs` shows. The places come from the file's text.
fn marked(hub: &mut Hub) {
    open_files(hub);
    let Some(text) = file_text(hub) else {
        return;
    };
    let doc = Doc::new(text, Lang::RUST, 1);
    let (Some((after, _)), Some((first, last)), Some((from, to))) = (
        find(&doc, 19, "left"),
        find(&doc, 20, "self.refill_at - now"),
        find(&doc, 4, "from_secs"),
    ) else {
        return;
    };
    let mut rows = server_rows();
    let warning = problem_rows().into_iter().nth(1).map(|row| {
        let mut row = row
            .text("path", PATH)
            .text("line", "20")
            .text("col", char_col(&doc, 20, first).to_string())
            .text("end_col", char_col(&doc, 20, last).to_string());
        row.id = RowId::new(format!("problem:{PATH}:20:0"));
        row
    });
    rows.extend(warning);
    let hint = char_col(&doc, 19, after + 4);
    rows.push(Row::new(format!("hint:{PATH}:19:{hint}")).text("text", ": u32"));
    rows.push(Row::new(format!("fix:{PATH}:20:0")).text("title", "Use saturating_duration_since"));
    let col = char_col(&doc, 4, from);
    rows.push(Row::new(format!("hover:{PATH}:4:{col}")).text("text", HOVER));
    for n in 0..REFERENCES {
        rows.push(Row::new(format!("ref:{PATH}:4:{col}:{n}")).text("path", PATH));
    }
    feed(hub, rows);
    let key = PaneCx {
        hub,
        pane: SCENE_PANE,
        focused: true,
        edits: None,
    }
    .key(PaneKind::Code);
    let file = File {
        surface: &key,
        path: PATH,
        doc: &doc,
    };
    let token = Token {
        line: LineNo::from_index(3),
        cols: Cols::new(from, to),
        follows: false,
    };
    let resting = marks::resting(&file, &token);
    if let Some(kept) = hub.panes.get_mut::<Kept>(None) {
        kept.resting = resting;
    }
}

/// The folders of a path faint, its file name bright and bold (record 4.3).
fn crumb(hub: &Hub) -> El {
    let tokens = &hub.model.tokens;
    let (folders, name) = PATH.rsplit_once('/').unwrap_or(("", PATH));
    let folders = folders
        .split('/')
        .flat_map(|folder| [folder, " / "])
        .collect::<String>();
    let bold = TextStyle {
        weight: SEMIBOLD,
        ..CODE_LINE
    };
    row()
        .gap(0.0)
        .child(text(folders).font(CODE_LINE).color(colour(tokens.faint)))
        .child(text(name).font(bold).color(colour(tokens.fg)))
}

/// The code viewer as frame 9 draws its middle pane, with a server in the header; the code
/// pane's own header has none.
fn code(win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    let context = PaneCx {
        hub,
        pane: SCENE_PANE,
        focused: true,
        edits: Some(&win.edits),
    };
    let key = context.key(PaneKind::Code);
    let tokens = &hub.model.tokens;
    let own = hub.def(PaneKind::Code).and_then(|def| def.tabs);
    let shown = own.map(|tabs| tabs(&context)).unwrap_or_default();
    let tab_bar = Bar {
        pane: SCENE_PANE,
        key: &key,
        tabs: &shown,
        focused: true,
        pulled: false,
        alone: true,
        closes: false,
        lights: None,
        at: At {
            origin: [0.0, 0.0],
            size: Size { w: 0.0, h: 0.0 },
        },
    };
    let table = hub.model.tables.get(FILES);
    let loaded = file_text(hub).map(|text| {
        let stamp = table.map_or(0, Rows::version);
        load(PATH, stamp, &text, Lang::RUST, None)
    });
    let surface = loaded.map(|loaded| {
        let marks = Rc::new(marks::of(&hub.model, PATH, &loaded.doc));
        let view = CodeView::new(
            key.as_str(),
            tokens,
            loaded.doc.clone(),
            loaded.colours.clone(),
        );
        let view = view.marks(marks);
        let bar_key = key.clone();
        match bar_line(hub, PATH) {
            Some(line) => view.slot(line, "fix", move |hub| bar(hub, &bar_key, PATH, line)),
            None => view,
        }
        .build()
    });
    let head = header(
        tokens,
        vec![glyph(&FILE, 11.0, tokens.faint), crumb(hub)],
        marks::server(hub, &key, PATH),
    )
    .key(format!("{key}.pane-header"));
    let warnings = text("1 warning").font(LABEL).color(colour(tokens.faint));
    let rule = text("All of this comes from the language server")
        .font(LABEL)
        .color(colour(tokens.faint));
    let foot = footer(tokens, vec![rule], vec![warnings]).key(format!("{key}.footer"));
    let body = col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(head)
        .children(surface)
        .child(foot);
    col()
        .w_full()
        .h_full()
        .child(tabs::bar(tokens, &tab_bar))
        .child(body)
        .children(card(hub, cx, &key))
}
