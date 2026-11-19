//! Files pane: changed files first, then the full tree; search behind a button.

mod body;
pub mod copy;
pub mod frame;
mod marks;
pub(crate) mod scenes;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use std::borrow::Cow;

use rune::{
    Curve, Cx, Easing, El, GrowthSpec, Icon, Key, KeyEvent, ScrollSpec, Tx, col, row, spacer, text,
};
use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::ids::{FeedId, PaneId};
use yd_core::kind::Kind;
use yd_core::pane::PaneKind;
use yd_core::tokens::Tokens;
use yd_ui::a11y;
use yd_ui::chrome::tabs::{Tab, Tabs};
use yd_ui::grid_view::view::open_file;
use yd_ui::kit::chip::{Chip, chip};
use yd_ui::kit::icons::{SEARCH, glyph};
use yd_ui::kit::key_hint::key_hint;
use yd_ui::kit::label::section;
use yd_ui::line_icon;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef};
use yd_ui::theme::look::{BODY, CODE, LABEL, META, NONE, colour};
use yd_ui::theme::scale::{LINE_DIVIDER, LINE_PICKED, ON_ACCENT};
use yd_ui::win::{Hub, Win};

use body::{Data, Item, data};

pub use marks::{Mark, marks};
use yd_ui::states::status::{Status, body};

use super::states::WORDS;

pub const FEED: FeedId = yd_core::feed::FILES;
pub const CHANGES: FeedId = yd_core::feed::CHANGES;

/// A row's file: `<pane key>.row.<path>` in the tree, `.row.change:<path>` under Changed.
fn thing_at(hub: &Hub, _: PaneId, key: &str) -> Option<Thing> {
    let (_, row) = key.split_once(".row.")?;
    let path = row.strip_prefix("change:").unwrap_or(row);
    Some(Thing::at(&hub.model, Address::new(Kind::File, path).ok()?))
}

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Files,
    view,
    scenes: scenes::SCENES,
    commands: &[],
    settings: &[],
    tabs: Some(tab),
    on_tab: None,
    finder: Some(Finder::Keyed(thing_at)),
};

/// The pane's one tab, with how many files there are and how many changed as its meta.
fn tab(pane: &PaneCx<'_>) -> Tabs {
    let tab = Tab {
        meta: meta(pane),
        ..Tab::of(PaneKind::Files)
    };
    Tabs {
        tabs: vec![tab],
        ..Tabs::default()
    }
}

static FOLDER: Icon = line_icon!(
    "folder",
    r#"<path d="M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2z"/>"#
);
static FILE: Icon = line_icon!(
    "file",
    r#"<path d="M14 3H7a2 2 0 00-2 2v14a2 2 0 002 2h10a2 2 0 002-2V8z"/><path d="M14 3v5h5"/>"#
);
static EYE: Icon = line_icon!(
    "eye",
    r#"<path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>"#
);
static PENCIL: Icon = line_icon!(
    "pencil",
    r#"<path d="M4 20h4L19 9l-4-4L4 16z"/><path d="M13.5 6.5l4 4"/>"#
);
static OPEN_FOLDER: Icon = line_icon!("chevron-down", r#"<path d="M6 9l6 6 6-6"/>"#);
static SHUT_FOLDER: Icon = line_icon!("chevron-right", r#"<path d="M9 6l6 6-6 6"/>"#);

/// The search box's opening: the mockup's slight overshoot.
const OPENING: Curve = Curve::tween(0.38, Easing::cubic(0.2, 1.5, 0.35, 1.0));

/// Which files stay bright; the rest fade to 40 %.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Filter {
    All,
    #[default]
    Touched,
    Edited,
}

/// The pane's own state; it travels with the pane.
#[derive(Debug, Default)]
pub struct State {
    /// The search box's text; none while the box is shut.
    pub search: Option<String>,
    pub filter: Filter,
    /// The selected row: its key past `.row.`, `change:<path>` in the flat list.
    pub selected: Option<String>,
    /// Folders the user opened or shut against their default.
    pub toggled: BTreeSet<String>,
}

impl State {
    pub fn query(&self) -> Option<&str> {
        self.search.as_deref()
    }
}

fn view(pane: &PaneCx<'_>, cx: &mut Cx<Win>) -> El {
    copy::under(pane, &pane.key(PaneKind::Files), listed(pane, cx))
}

fn listed(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Files);
    let (tokens, model) = (pane.tokens(), pane.model());
    let fresh = State::default();
    let state = pane.state::<State>().unwrap_or(&fresh);
    let folder = |thread| yd_ui::shown::works_in(pane.hub, thread);
    let marks = pane
        .thread()
        .map(|thread| marks::marks(thread, folder(thread)));
    let marks = marks.unwrap_or_default();
    let data = Rc::new(data(
        model.tables.get(FEED),
        model.tables.get(CHANGES),
        marks,
        state,
    ));
    if data.files == 0 && data.changes.is_empty() {
        let status = Status::of_table(model.tables.get(FEED)).unwrap_or(Status::Empty);
        return body(pane, PaneKind::Files, &status, &WORDS);
    }
    let filters = [
        (Filter::All, "All".to_owned()),
        (
            Filter::Touched,
            format!("Touched · {}", data.count(Filter::Touched)),
        ),
        (
            Filter::Edited,
            format!("Edited · {}", data.count(Filter::Edited)),
        ),
    ];
    let open = state.search.is_some();
    let cut = data.cut.clone().map(|why| {
        row()
            .key(format!("{key}.cut"))
            .h(20.0)
            .px(tokens.gutter)
            .center_y()
            .child(text(why).font(META).color(colour(tokens.faint)).truncate())
    });
    let filter_row = row()
        .key(format!("{key}.filters"))
        .h(20.0)
        .mt(4.0)
        .px(tokens.gutter)
        .gap(6.0)
        .center_y()
        .children(filters.into_iter().map(|(filter, words)| {
            let (look, id) = (
                if filter == state.filter {
                    Chip::Pill
                } else {
                    Chip::Plain
                },
                pane.pane,
            );
            chip(tokens, look, &words, None, false)
                .key(format!("{key}.filter.{filter:?}"))
                .on_press(move |win: &mut Win| change(win, id, |s| s.filter = filter))
        }))
        .child(spacer())
        .child(search_button(tokens, &key, pane.pane, open));
    col()
        .key(key.clone())
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .child(filter_row)
        .child(search_box(tokens, &key, pane.pane, state))
        .child(list(tokens, &key, pane.pane, data, state))
        .children(cut)
        .children(
            state
                .query()
                .is_some_and(|q| !q.is_empty())
                .then(|| hints(tokens, &key)),
        )
}

/// Changes this pane's state from a handler.
fn change(win: &mut Win, pane: PaneId, edit: impl FnOnce(&mut State)) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(state) = hub.panes.get_mut::<State>(Some(pane))
    {
        edit(state);
    }
}

fn search_button(tokens: &Tokens, key: &str, pane: PaneId, open: bool) -> El {
    let (fill, ink) = if open {
        (tokens.accent, ON_ACCENT.of(tokens))
    } else {
        (tokens.raised, tokens.fg)
    };
    let field = format!("{key}.search-box");
    row()
        .key(format!("{key}.search"))
        .w(24.0)
        .h(20.0)
        .center()
        .radius(6.0)
        .fill(colour(fill))
        .help("Search")
        .label("Search")
        .press_scale(0.95)
        .child(glyph(&SEARCH, 12.0, ink))
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
            tx.motion_curve(OPENING);
            let mut opened = false;
            change(win, pane, |s| {
                s.search = if s.search.is_some() {
                    None
                } else {
                    Some(String::new())
                };
                opened = s.search.is_some();
            });
            if opened {
                tx.focus(field.clone(), false);
            }
        })
}

/// The box is always laid out; shut, it is 0 tall and clipped, so it opens by growing.
fn search_box(tokens: &Tokens, key: &str, pane: PaneId, state: &State) -> El {
    let query = state.query().unwrap_or("");
    let words = if query.is_empty() {
        text("Find a file, or a name inside one")
            .font(LABEL)
            .color(colour(tokens.faint))
    } else {
        text(query).font(LABEL).color(colour(tokens.fg))
    };
    let caret = col().w(1.0).h(12.0).fill(colour(tokens.accent));
    let (height, top, bottom) = if state.search.is_some() {
        (22.0, 4.0, 2.0)
    } else {
        (0.0, 0.0, 0.0)
    };
    row()
        .key(format!("{key}.search-box"))
        .h(height)
        .mt(top)
        .mb(bottom)
        .mx(6.0)
        .px(7.0)
        .gap(6.0)
        .center_y()
        .radius(6.0)
        .clip_rounded(6.0)
        .fill(colour(tokens.raised))
        .focusable(true)
        .on_key(move |win: &mut Win, event: &KeyEvent, tx: &mut Tx| typed(win, pane, event, tx))
        .child(glyph(&SEARCH, 11.0, tokens.faint))
        .children(query.is_empty().then(|| caret.clone()))
        .child(words)
        .children((!query.is_empty()).then_some(caret))
        .child(spacer())
        .child(key_hint(tokens, "esc"))
}

/// A key in the search box: letters type, Backspace takes one back, Esc shuts the box.
fn typed(win: &mut Win, pane: PaneId, event: &KeyEvent, tx: &mut Tx) -> bool {
    let mods = event.mods;
    match &event.key {
        Key::Text(words) if !mods.cmd && !mods.ctrl => change(win, pane, |s| {
            if let Some(query) = s.search.as_mut() {
                query.push_str(words);
            }
        }),
        Key::Backspace => change(win, pane, |s| {
            if let Some(query) = s.search.as_mut() {
                query.pop();
            }
        }),
        Key::Escape => {
            tx.motion_curve(OPENING);
            change(win, pane, |s| s.search = None);
        }
        _ => return false,
    }
    true
}

fn hints(tokens: &Tokens, key: &str) -> El {
    let hint = |words: &str| text(words).font(LABEL).color(colour(tokens.faint));
    row()
        .key(format!("{key}.hints"))
        .h(27.0)
        .px(10.0)
        .gap(12.0)
        .center_y()
        .border_sides([1.0, 0.0, 0.0, 0.0], colour(LINE_DIVIDER.of(tokens)))
        .child(hint("↵ open"))
        .child(hint("⌘↵ attach"))
        .child(hint("esc back to the tree"))
}

/// Everything below the box on virtual rows: sections, changed files, the tree.
fn list(tokens: &Tokens, key: &str, pane: PaneId, data: Rc<Data>, state: &State) -> El {
    let (selected, filter) = (state.selected.clone(), state.filter);
    let mut version = DefaultHasher::new();
    (&data.items, &selected, filter).hash(&mut version);
    let rows = Rc::new(Rows {
        tokens: tokens.clone(),
        key: key.to_owned(),
        pane,
        data,
        selected,
        filter,
    });
    let (keyed, built) = (rows.clone(), rows.clone());
    let stack = col()
        .key(format!("{key}.stack"))
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            rows.data.items.len(),
            tokens.row,
            move |_: &Win, at| Cow::Owned(keyed.key_at(at)),
            move |_: &Win, at| built.row_at(at),
        )
        .virtual_rows_version(version.finish());
    a11y::list(col(), "Files")
        .key(format!("{key}.rows"))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(stack)
}

/// What the row closures read: they run after the view has returned.
struct Rows {
    tokens: Tokens,
    key: String,
    pane: PaneId,
    data: Rc<Data>,
    selected: Option<String>,
    filter: Filter,
}

impl Rows {
    fn key_at(&self, at: usize) -> String {
        match self.data.items.get(at) {
            Some(Item::Section { words, .. }) => [&self.key, ".section.", words].concat(),
            Some(Item::Flat(path)) => [&self.key, ".row.change:", path].concat(),
            Some(Item::Node { path, .. }) => [&self.key, ".row.", path].concat(),
            None => [&self.key, ".gone"].concat(),
        }
    }

    fn row_at(&self, at: usize) -> El {
        let tokens = &self.tokens;
        let key = self.key_at(at);
        match self.data.items.get(at) {
            Some(Item::Section { words, note }) => {
                section(tokens, words, None, *note).key(key).mt(8.0)
            }
            Some(Item::Flat(path)) => {
                let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
                let sub = text(folder)
                    .font(META)
                    .color(colour(tokens.faint))
                    .truncate()
                    .min_w(0.0);
                self.line(key, path, 0, None, name)
                    .opacity(if self.data.bright(path, self.filter) {
                        1.0
                    } else {
                        0.4
                    })
                    .child(sub)
                    .child(spacer())
                    .children(self.marks(path, true))
            }
            Some(Item::Node {
                path,
                name,
                depth,
                dir,
                open,
            }) => {
                let fold = if *dir { Some(*open) } else { None };
                let bright = if *dir {
                    self.data.bright_under(path, self.filter)
                } else {
                    self.data.bright(path, self.filter)
                };
                let line = self
                    .line(key, path, *depth, Some(fold), name)
                    .child(spacer());
                let line = if *dir {
                    line
                } else {
                    line.children(self.marks(path, false))
                };
                if bright { line } else { line.opacity(0.4) }
            }
            None => col(),
        }
    }

    /// A 20 tall row: indent, chevron column in the tree, icon, name.
    fn line(
        &self,
        key: String,
        path: &str,
        depth: u32,
        fold: Option<Option<bool>>,
        name: &str,
    ) -> El {
        let tokens = &self.tokens;
        let id = if fold.is_none() {
            ["change:", path].concat()
        } else {
            path.to_owned()
        };
        let on = self.selected.as_deref() == Some(id.as_str());
        let indent = 8.0 + 12.0 * depth as f32;
        let chevron = fold.map(|open| {
            let drawing = open.map(|open| if open { &OPEN_FOLDER } else { &SHUT_FOLDER });
            row()
                .w(10.0)
                .children(drawing.map(|d| glyph(d, 10.0, tokens.faint)))
        });
        let drawing = if matches!(fold, Some(Some(_))) {
            &FOLDER
        } else {
            &FILE
        };
        let ink = if on { tokens.fg } else { tokens.dim };
        let (pane, path, dir) = (self.pane, path.to_owned(), matches!(fold, Some(Some(_))));
        a11y::item(row(), tokens, on)
            .key(key)
            .h(tokens.row)
            .pl(indent)
            .pr(8.0)
            .gap(5.0)
            .center_y()
            .radius(4.0)
            .fill(colour(if on { tokens.selected } else { NONE }))
            .border_sides(
                [0.0, 0.0, 0.0, if on { 2.0 } else { 0.0 }],
                colour(LINE_PICKED.of(tokens)),
            )
            .press_scale(0.95)
            .children(chevron)
            .child(glyph(drawing, 11.0, ink))
            .child(
                text(name)
                    .font(BODY)
                    .color(colour(ink))
                    .truncate()
                    .min_w(0.0),
            )
            .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
                pressed(win, tx, pane, &id, &path, dir);
            })
    }

    /// The marks at a row's right: writing now, read, edited, and in the flat list the counts.
    fn marks(&self, path: &str, counts: bool) -> Vec<El> {
        let tokens = &self.tokens;
        let mark = self.data.marks.get(path).copied().unwrap_or_default();
        let words = |w: &str, ink| text(w).font(META).color(colour(ink));
        let mut out = Vec::new();
        if mark.writing {
            out.push(words("writing now", tokens.need));
        } else {
            out.extend((mark.reads > 0).then(|| glyph(&EYE, 10.0, tokens.faint)));
            out.extend(mark.edited.then(|| glyph(&PENCIL, 10.0, tokens.faint)));
            out.extend(mark.word().map(|w| words(&w, tokens.faint)));
        }
        if let Some(change) = self.data.change(path).filter(|_| counts) {
            let count = |sign: &str, n: &str, ink| {
                text([sign, n].concat())
                    .font(META.family(CODE))
                    .color(colour(ink))
            };
            out.extend((change.added != "0").then(|| count("+", &change.added, tokens.ok)));
            out.extend((change.removed != "0").then(|| count("−", &change.removed, tokens.bad)));
        }
        out
    }
}

/// A press on a row: a folder opens or shuts, a file is selected and opened in the viewer.
fn pressed(win: &mut Win, tx: &mut Tx, pane: PaneId, id: &str, path: &str, dir: bool) {
    change(win, pane, |s| {
        if !dir {
            s.selected = Some(id.to_owned());
        } else if !s.toggled.remove(path) {
            s.toggled.insert(path.to_owned());
        }
    });
    if let (false, Ok(file)) = (dir, Address::new(Kind::File, path)) {
        open_file(win, tx, file);
    }
}

/// The meta a pane's tab shows: "214 files · 3 changed".
pub fn meta(pane: &PaneCx<'_>) -> String {
    let model = pane.model();
    let data = data(
        model.tables.get(FEED),
        model.tables.get(CHANGES),
        Default::default(),
        &State::default(),
    );
    format!("{} files · {} changed", data.files, data.changes.len())
}
