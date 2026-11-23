//! Code viewer pane: tabs for the open files, the breadcrumb and one file on the code surface.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use rune::{Cx, El, Tx, col, el, row, text};
use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::command::run::{PREVIEW, opened, previewed, shown_path};
use yd_core::command::table::{Command, OPEN_FILE, Route, Run, Target};
use yd_core::command::tools::lsp::REFERENCES;
use yd_core::feed::{FILES, FeedEvent, FeedStatus, LSP, OPEN_FILES, SourceCmd, Table};
use yd_core::ids::{PaneId, RowId, TabId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Rows, Value};
use yd_core::tokens::Tokens;
use yd_ui::chrome::tabs::{Tab, Tabs};
use yd_ui::code::colour::{Loaded, Wake, load};
use yd_ui::code::doc::{Doc, Lang, LineNo};
use yd_ui::code::marks::{Marks, Tint};
use yd_ui::code::pointer::CodeEvent;
use yd_ui::code::rows::{InView, Lines, line_of, thing};
use yd_ui::code::view::{CodeView, Gutter};
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef, TabAsk};
use yd_ui::theme::look::{CODE_LINE, META, colour, white};
use yd_ui::win::{Hub, Win, ask, dispatch};

use super::refs;
use crate::lsp::marks;
use yd_ui::states::status::Status;

use super::states::WORDS;

mod scenes;
#[cfg(test)]
mod tests;

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Code,
    view,
    scenes: scenes::SCENES,
    commands: &[
        Command {
            id: OPEN_FILE,
            title: "Open in Code Viewer",
            key: None,
            menu: None,
            kinds: &[Kind::File, Kind::Lines],
            route: Route::Model,
        },
        Command {
            id: REFERENCES,
            title: "Find References",
            key: None,
            menu: None,
            kinds: &[Kind::Lines],
            route: Route::Source(LSP),
        },
    ],
    settings: &[],
    tabs: Some(file_tabs),
    on_tab: Some(on_tab),
    finder: Some(Finder::Keyed(thing_at)),
};

/// A panel between two lines of the open file.
#[derive(Debug, Clone, Copy)]
pub struct Slot {
    pub after: LineNo,
    /// Names the slot's row: `<pane key>.slot.<name>`.
    pub name: &'static str,
    pub build: fn(&Hub, PaneId) -> El,
}

/// What other parts of the app put on this pane's open file, and what its reader selected.
#[derive(Default)]
pub struct Viewer {
    /// Marks for the file at this path: a language server's, a debugger's, names to follow.
    pub marks: Option<(String, Rc<Marks>)>,
    pub slot: Option<Slot>,
    pub gutter: Gutter,
    selected: Option<(RowId, Lines)>,
}

/// The file a tab row stands for.
fn path(tab: &Row) -> &str {
    tab.words("path").unwrap_or_default()
}

/// A path's last part, which names its tab.
fn name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or_default()
}

fn tabs(table: &Table) -> impl Iterator<Item = &Row> {
    (0..table.len()).filter_map(|at| table.get(at))
}

/// The tab that shows: the one opened last.
fn current(table: &Table) -> Option<&Row> {
    tabs(table).max_by_key(|tab| opened(tab))
}

/// Opens `file` from a handler: runs `code.open`, which makes its tab the one that shows
/// and asks the files source for its text.
pub fn show(win: &mut Win, file: &Address) {
    let run = Run::new(OPEN_FILE).on(Target::Thing(file.clone()));
    dispatch(win, Msg::Command(Route::Model, run));
}

/// The number `code.open` gave an open file's row: its tab's id for as long as it is open.
fn tab_id(tab: &Row) -> Option<TabId> {
    tab.words("tab")?.parse().ok().map(TabId)
}

/// The pane's tabs for its bar: one an open file, the one opened last in front with its
/// facts as the meta.
fn file_tabs(pane: &PaneCx<'_>) -> Tabs {
    let Some(table) = pane.model().tables.get(OPEN_FILES) else {
        return Tabs::default();
    };
    let shown = Shown::of(pane.hub, pane.pane);
    // A pane about one file has that file as its one tab, whatever else is open.
    if let Some(shown) = shown.as_ref().filter(|shown| shown.about) {
        let tab = Tab {
            title: name(shown.path).to_owned(),
            meta: shown.meta(pane.hub),
            ..Tab::of(PaneKind::Code)
        };
        return Tabs {
            active: Some(tab.id),
            tabs: vec![tab],
            adds: false,
            tool: None,
        };
    }
    let front = shown.as_ref().map(|shown| &shown.id);
    let files = tabs(table).filter_map(|file| {
        let name = name(path(file));
        let meta = shown.as_ref().filter(|_| front == Some(&file.id));
        Some(Tab {
            id: tab_id(file)?,
            title: name.to_owned(),
            meta: meta.map(|shown| shown.meta(pane.hub)).unwrap_or_default(),
            preview: previewed(file),
            ..Tab::of(PaneKind::Code)
        })
    });
    Tabs {
        tabs: files.collect(),
        active: shown.as_ref().and_then(|shown| tab_id(shown.tab?)),
        adds: false,
        tool: None,
    }
}

/// What the bar asks: a file in front, or its tab closed. Its text stays with the files
/// source until that drops it. The tab in front, pressed, is kept: a look no longer.
fn on_tab(win: &mut Win, _: PaneId, ask: TabAsk) {
    let (tab, close) = match ask {
        TabAsk::Show(tab) => (tab, false),
        TabAsk::Close(tab) => (tab, true),
        TabAsk::New => return,
    };
    let file = win.hub.try_borrow().ok().and_then(|hub| {
        let table = hub.model.tables.get(OPEN_FILES)?;
        let file = tabs(table).find(|file| tab_id(file) == Some(tab))?;
        let front = current(table).is_some_and(|front| front.id == file.id);
        let address = Address::new(Kind::File, path(file)).ok()?;
        Some((file.id.clone(), address, front))
    });
    match file {
        Some((id, ..)) if close => dispatch(win, Msg::Feed(OPEN_FILES, FeedEvent::Remove(id))),
        Some((_, file, true)) => show(win, &file),
        Some((_, file, false)) => {
            let look = Run {
                text: Some(PREVIEW.to_owned()),
                ..Run::new(OPEN_FILE).on(Target::Thing(file))
            };
            dispatch(win, Msg::Command(Route::Model, look));
        }
        None => {}
    }
}

/// The file a pane shows, and its text once the files source has read it.
struct Shown<'a> {
    id: RowId,
    path: &'a str,
    /// Its row among the open files; a file the pane is about may have none.
    tab: Option<&'a Row>,
    /// Whether the pane is about this file, and so shows no other.
    about: bool,
    loaded: Option<Loaded>,
}

impl<'a> Shown<'a> {
    fn of(hub: &'a Hub, pane: PaneId) -> Option<Shown<'a>> {
        let file = shown_path(&hub.model, pane)?;
        let open = hub.model.tables.get(OPEN_FILES);
        let tab = open.and_then(|open| tabs(open).find(|tab| path(tab) == file));
        let id = tab.map_or_else(|| RowId::new(format!("file:{file}")), |tab| tab.id.clone());
        let about = hub.model.grid.thing(pane).is_some();
        let files = hub.model.tables.get(FILES);
        let read = files.and_then(|files| Some((files.version(), files.find(&id)?)));
        let loaded = read.and_then(|(stamp, read)| {
            // A long file is coloured on a worker, which wakes the first window when done.
            let wake = hub.wins.first().map(|(_, app)| {
                let app = app.clone();
                Box::new(move || app.update(|_, _| {})) as Wake
            });
            let lang = Lang::for_path(file);
            Some(load(id.as_str(), stamp, read.words("text")?, lang, wake))
        });
        Some(Shown {
            id,
            path: file,
            tab,
            about,
            loaded,
        })
    }

    fn is_new(&self) -> bool {
        let new = |tab: &Row| tab.get("new") == Some(&Value::Flag(true));
        self.tab.is_some_and(new)
    }

    /// The header's facts: `Rust · 23 lines · new · 1 error`.
    fn meta(&self, hub: &Hub) -> String {
        let mut facts = vec![Lang::for_path(self.path).title().to_owned()];
        facts.extend(
            self.loaded
                .as_ref()
                .map(|loaded| format!("{} lines", loaded.doc.lines())),
        );
        facts.extend(self.is_new().then(|| "new".to_owned()));
        facts.extend(marks::counted(&hub.model, self.path));
        facts.join(" · ")
    }

    /// The marks others put on this file under the green bars of a file new in this thread,
    /// and on those what the language server says of it.
    fn marks(&self, hub: &Hub, viewer: Option<&Viewer>) -> Rc<Marks> {
        let theirs = viewer
            .and_then(|viewer| viewer.marks.as_ref())
            .filter(|(file, _)| file == self.path)
            .map(|(_, marks)| (**marks).clone());
        let base = theirs.unwrap_or_default();
        let base = match self.is_new() {
            true => base.tint_all(Tint::Added),
            false => base,
        };
        Rc::new(match &self.loaded {
            Some(loaded) => marks::onto(base, &hub.model, self.path, &loaded.doc),
            None => base,
        })
    }
}

/// What a keyed element of this pane's surface stands for: a row of the references panel,
/// the lines the reader selected when the key is one of them, else the key's own line.
pub fn thing_at(hub: &Hub, pane: PaneId, key: &str) -> Option<Thing> {
    if let Some(reference) = refs::thing_at(hub, pane, key) {
        return Some(reference);
    }
    let shown = Shown::of(hub, pane)?;
    let line = line_of(key)?;
    let viewer = hub.panes.get::<Viewer>(Some(pane));
    let selected = viewer
        .and_then(|viewer| viewer.selected.as_ref())
        .filter(|(file, lines)| *file == shown.id && lines.contains(line))
        .map(|(_, lines)| *lines);
    let lines = selected.unwrap_or(Lines::new(line, line));
    thing(shown.path, &shown.loaded?.doc, lines)
}

/// The file a surface was built for, kept by the handler that hears it.
struct Open {
    pane: PaneId,
    id: RowId,
    key: String,
    path: String,
    doc: Arc<Doc>,
}

/// What the surface told this pane: the references panel hears it first, then the language
/// server's marks. The view draws nothing from the selection, so hearing of it runs no view.
fn heard(win: &mut Win, open: &Open, event: CodeEvent, tx: &mut Tx) {
    if refs::heard(win, tx, open.pane, event) {
        return;
    }
    let file = marks::File {
        surface: &open.key,
        path: &open.path,
        doc: &open.doc,
    };
    if marks::heard(win, &file, event, tx) {
        return;
    }
    match event {
        CodeEvent::Selection(lines) => {
            if let Ok(mut hub) = win.hub.try_borrow_mut()
                && let Some(viewer) = hub.panes.get_mut::<Viewer>(Some(open.pane))
            {
                viewer.selected = lines.map(|lines| (open.id.clone(), lines));
            }
            tx.live_only();
        }
        CodeEvent::HoverStart(_)
        | CodeEvent::HoverEnd(_)
        | CodeEvent::Press(_)
        | CodeEvent::CommandPress(_)
        | CodeEvent::GutterPress(_)
        | CodeEvent::SecondaryPress { .. } => {}
    }
}

/// The path of the open file (record 4.3): its folders in faint, its name in bright; at the
/// right the server that reads it, by its dot and name (record 6.4.2).
fn breadcrumb(tokens: &Tokens, key: &str, path: &str, server: Vec<El>) -> El {
    let (folders, name) = path.rsplit_once('/').unwrap_or(("", path));
    let folders: String = folders
        .split('/')
        .filter(|folder| !folder.is_empty())
        .flat_map(|folder| [folder, " / "])
        .collect();
    row()
        .key(format!("{key}.breadcrumb"))
        .w_full()
        .h(20.0)
        .mb(2.0)
        .px(tokens.gutter)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], colour(white(tokens.inner_lines)))
        .clip_rounded(0.0)
        .child(text(folders).font(CODE_LINE).color(colour(tokens.faint)))
        .child(text(name).font(CODE_LINE).color(colour(tokens.fg)))
        .child(server_part(server))
}

/// The server's dot and name at the right; a reason it gives fills the room before them.
fn server_part(server: Vec<El>) -> El {
    let part = row().grow(1.0).min_w(0.0).pl(12.0).gap(6.0).center_y();
    part.justify_end().children(server)
}

/// Tells the language server of the open file: asked when the file first shows and when its
/// text changes on disk, since a server reads an open file from what it was told.
fn told(key: &str, path: &str, rev: u64) -> El {
    let file = path.to_owned();
    el().key(format!("{key}.opened.{path}.{rev}.ask"))
        .size(0.0, 0.0)
        .on_layout_tx(move |win: &mut Win, _, _: &mut Tx| marks::open(win, &file, rev))
}

/// The open file on the code surface, with what the pane's state and the language server
/// put on it: the references panel under its line, the quick-fix bar under its own.
fn surface(pane: &PaneCx<'_>, key: &str, shown: &Shown<'_>, wide: Option<f32>) -> Option<El> {
    let loaded = shown.loaded.as_ref()?;
    let viewer = pane.state::<Viewer>();
    let id = pane.pane;
    let open = Open {
        pane: id,
        id: shown.id.clone(),
        key: key.to_owned(),
        path: shown.path.to_owned(),
        doc: loaded.doc.clone(),
    };
    let fix = marks::bar_line(pane.hub, &open.path);
    let (shown_path, shown_doc) = (open.path.clone(), open.doc.clone());
    let (bar_key, file) = (open.key.clone(), open.path.clone());
    let view = CodeView::new(
        key,
        pane.tokens(),
        loaded.doc.clone(),
        loaded.colours.clone(),
    )
    .marks(shown.marks(pane.hub, viewer))
    .gutter(viewer.map(|viewer| viewer.gutter).unwrap_or_default())
    .on_event(move |win, event, tx| heard(win, &open, event, tx))
    .on_visible(move |win, lines| {
        if let Ok(mut hub) = win.hub.try_borrow_mut()
            && let Some(seen) = hub.panes.get_mut::<InView>(Some(id))
        {
            seen.0 = Some((shown_path.clone(), shown_doc.clone(), lines));
        }
    });
    // Incident: a slot is as wide as the file's longest line, so in a narrow pane the peek
    // panel's Attach all lay past the pane's edge. A slot is held to the width in view.
    let fit = move |panel: El| match wide {
        Some(wide) => row().w(wide).child(panel),
        None => panel,
    };
    let view = match viewer.and_then(|viewer| viewer.slot) {
        Some(slot) => view.slot(slot.after, slot.name, move |hub| fit((slot.build)(hub, id))),
        None => view,
    };
    let view = match fix {
        Some(line) => view.slot(line, "fix", move |hub| {
            fit(marks::bar(hub, &bar_key, &file, line))
        }),
        None => view,
    };
    Some(view.build())
}

/// The most of one file the files source reads (its `MAX_TEXT`); the row says when it cut.
const READ_CAP_MIB: u32 = 2;

/// The files source's cut, said where the text stops being whole: what was kept and how to
/// read the rest.
fn cut_row(tokens: &Tokens, key: &str, file: &Row) -> El {
    let lines = file.words("lines").unwrap_or("0");
    let said = format!(
        "[…] first {lines} lines shown: the files source reads {READ_CAP_MIB} MiB of a file at most; open it in an editor for the rest"
    );
    row()
        .key(format!("{key}.cut"))
        .w_full()
        .h(20.0)
        .px(tokens.gutter)
        .center_y()
        .child(text(said).font(META).color(colour(tokens.faint)).truncate())
}

fn view(pane: &PaneCx<'_>, cx: &mut Cx<Win>) -> El {
    let tokens = pane.tokens();
    let key = pane.key(PaneKind::Code);
    let shown = Shown::of(pane.hub, pane.pane);
    let file = shown.as_ref().map(|shown| shown.path);
    let crumb =
        file.map(|file| breadcrumb(tokens, &key, file, marks::server(pane.hub, &key, file)));
    let wide = cx.layout_size(&format!("{key}.code")).map(|[wide, _]| wide);
    let files = pane.model().tables.get(FILES);
    let read = shown.as_ref().and_then(|shown| files?.find(&shown.id));
    // Invariant: a file's text is asked for when its tab opens. A file a pane is about and
    // no tab holds was asked for by nothing, so the pane asks.
    let unread = shown
        .as_ref()
        .filter(|shown| read.is_none() && shown.tab.is_none());
    let asked = unread.map(|shown| {
        let file = PathBuf::from(shown.path);
        ask(format!("{key}.unread.{}.ask", shown.path), Msg::Source(FILES, SourceCmd::Read(file)))
    });
    let cut = read
        .filter(|row| row.get("truncated") == Some(&Value::Flag(true)))
        .map(|row| cut_row(tokens, &key, row));
    let body = shown
        .as_ref()
        .and_then(|shown| surface(pane, &key, shown, wide))
        .unwrap_or_else(|| {
            // No file open is empty; an open file whose text has not come is the files source's.
            let failed = read.and_then(|row| row.words("error"));
            let status = match (shown.as_ref(), failed) {
                (None, _) => Status::Empty,
                (Some(_), Some(why)) => Status::Error(why.to_owned()),
                (Some(_), None) => match pane.model().tables.get(FILES).map(Table::status) {
                    None | Some(FeedStatus::Live | FeedStatus::Empty) => Status::Loading,
                    Some(status) => Status::of_feed(status),
                },
            };
            yd_ui::states::status::view(pane, &key, &status, &WORDS)
        });
    let pane_col = col()
        .key(key.clone())
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .children(crumb)
        .children(cut)
        .child(body)
        .children(shown.as_ref().and_then(|shown| {
            let rev = shown.loaded.as_ref()?.doc.rev();
            Some(told(&key, shown.path, rev))
        }))
        .children(asked)
        .children(marks::card(pane.hub, cx, &key));
    crate::files::pane::copy::under(pane, &key, pane_col)
}
