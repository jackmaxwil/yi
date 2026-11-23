//! The list view: any list drawn from its description. A header bar, column titles, the
//! feed's state, the rows in a scroll viewport that builds only what shows, a footer line.

use std::borrow::Cow;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use rune::{
    Color, El, GrowthSpec, KeyEvent, ScrollSpec, TextCase, TextStyle, Tx, col, el, row, text,
};
use yd_core::feed::Table;
use yd_core::ids::{FeedId, PaneId, RowId};
use yd_core::list::{Col, ListDesc, RowHeight};
use yd_core::model::Model;
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Rows, Tone};
use yd_core::tokens::{ThemeValue, Tokens};

use super::row::{self, GAP, fit, words};
use super::state::{ListState, Lists};
use super::{fit as columns, keys};
use crate::kit::bars::{header, lead};
use crate::pane::{PaneCx, Scene};
use crate::states::status::{self as states, Said, Status, Words, notice};
use crate::theme::look::{META, colour, white};
use crate::theme::scale::{BAR_QUIET, BAR_TRACK, LINE_PICKED, LINE_TREE, WELL};
use crate::win::{Hub, Win};

mod demo;
mod order;

use order::Order;

pub const SCENES: &[Scene] = demo::SCENES;

#[cfg(test)]
thread_local! {
    /// How many row keys the lists of this thread were asked for, for the long-list test.
    pub(super) static KEYS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A column title: uppercase, tracked (record 5.1.4).
const TITLE: TextStyle = TextStyle {
    weight: 500.0,
    tracking: 0.57,
    ..META
};

/// Token values as Rune colours and sizes, read once for each run of the view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Look {
    pub fg: Color,
    pub dim: Color,
    pub faint: Color,
    pub accent: Color,
    pub need: Color,
    pub ok: Color,
    pub bad: Color,
    pub warn: Color,
    pub model: Color,
    pub context: Color,
    pub selected: Color,
    /// The keyboard ring: the theme's own, drawn inside the viewport that fills its pane.
    pub ring: Color,
    pub hairline: Color,
    pub well: Color,
    pub bar_quiet: Color,
    pub bar_track: Color,
    pub tree: Color,
    pub picked: Color,
    pub row: f32,
    pub line: f32,
    pub gutter: f32,
}

impl Look {
    pub(super) fn of(tokens: &Tokens) -> Self {
        Self {
            fg: colour(tokens.fg),
            dim: colour(tokens.dim),
            faint: colour(tokens.faint),
            accent: colour(tokens.accent),
            need: colour(tokens.need),
            ok: colour(tokens.ok),
            bad: colour(tokens.bad),
            warn: colour(tokens.warn),
            model: colour(tokens.model),
            context: colour(tokens.context),
            selected: colour(tokens.selected),
            ring: crate::a11y::ring_colour(tokens),
            hairline: colour(white(tokens.inner_lines)),
            well: colour(WELL.of(tokens)),
            bar_quiet: colour(BAR_QUIET.of(tokens)),
            bar_track: colour(BAR_TRACK.of(tokens)),
            tree: colour(LINE_TREE.of(tokens)),
            picked: colour(LINE_PICKED.of(tokens)),
            row: tokens.row,
            line: tokens.line,
            gutter: tokens.gutter,
        }
    }

    pub(super) fn tone(&self, tone: Tone) -> Color {
        match tone {
            Tone::Fg => self.fg,
            Tone::Dim => self.dim,
            Tone::Faint => self.faint,
            Tone::Accent => self.accent,
            Tone::Need => self.need,
            Tone::Ok => self.ok,
            Tone::Bad => self.bad,
            Tone::Warn => self.warn,
            Tone::Model => self.model,
            Tone::Context => self.context,
        }
    }
}

/// Where a list's rows are found again when a row is built, after the view has returned.
enum Source {
    /// A feed's own table in the model, named after its pane.
    Feed(FeedId),
    /// A table nothing can find again, such as one a pane made in its view: the list's copy.
    Copy(Rc<Table>),
}

impl Source {
    fn of(pane: &PaneCx<'_>, table: &Table) -> Self {
        // Invariant: rows are built after the view returns, so the table must be found again.
        // A table in the model is found by its feed, any instance; any other is copied.
        match pane.model().tables.feed_of(table) {
            Some(feed) => Source::Feed(feed),
            None => Source::Copy(Rc::new(table.clone())),
        }
    }

    fn table<'a>(&'a self, model: &'a Model) -> Option<&'a Table> {
        match self {
            Source::Feed(feed) => model.tables.get(*feed),
            Source::Copy(table) => Some(table),
        }
    }
}

/// What the rows and the handlers of one list share for one run of the view.
pub(super) struct Frame {
    pub(super) pane: PaneId,
    /// Which list of its pane this is: `""` for a pane that is one list.
    pub(super) part: &'static str,
    /// The list's key, the start of every key in it.
    pub(super) key: String,
    pub(super) desc: ListDesc,
    pub(super) look: Look,
    source: Source,
    order: Order,
    /// Whether the list keeps its newest row in view, as a log does.
    pub(super) follow: bool,
    /// The columns this width has room for, a bit each.
    pub(super) kept: u32,
    /// Moves when anything a kept row was drawn from does.
    stamp: u64,
    open: Option<RowId>,
    cursor: Option<RowId>,
    /// What the open row's buttons last said.
    pub(super) said: Option<String>,
}

impl Frame {
    pub(super) fn row_height(&self) -> f32 {
        match self.desc.height {
            RowHeight::Row => self.look.row,
            RowHeight::Line => self.look.line,
        }
    }

    /// The key of the scroll viewport, which also takes the list's keys.
    pub(super) fn rows_key(&self) -> String {
        format!("{}.rows", self.key)
    }

    pub(super) fn stamp(&self) -> u64 {
        self.stamp
    }

    pub(super) fn is_open(&self, row: &RowId) -> bool {
        self.open.as_ref() == Some(row)
    }

    pub(super) fn is_cursor(&self, row: &RowId) -> bool {
        self.cursor.as_ref() == Some(row)
    }

    /// The row drawn at `slot` and its depth in the tree.
    pub(super) fn at<'a>(&'a self, hub: &'a Hub, slot: usize) -> Option<(&'a Row, u8)> {
        self.order.at(self.source.table(&hub.model)?, slot)
    }

    /// How many rows the list draws, and the slot of `row`, looked for at `hint` first.
    pub(super) fn place(
        &self,
        hub: &Hub,
        row: Option<(&RowId, Option<usize>)>,
    ) -> Option<(usize, Option<usize>)> {
        let table = self.source.table(&hub.model)?;
        let slot = row.and_then(|(row, hint)| self.order.slot_of(table, row, hint));
        Some((self.order.len(table), slot))
    }

    /// Changes this list's state from a handler. `None` when the hub is in use.
    pub(super) fn change<R>(
        &self,
        win: &mut Win,
        change: impl FnOnce(&mut ListState) -> R,
    ) -> Option<R> {
        let mut hub = win.hub.try_borrow_mut().ok()?;
        let lists = hub.panes.get_mut::<Lists>(Some(self.pane))?;
        Some(change(lists.list_mut(self.part)))
    }

    fn key_at(&self, win: &Win, slot: usize) -> String {
        #[cfg(test)]
        KEYS.set(KEYS.get().saturating_add(1));
        let found = win.hub.try_borrow().ok().and_then(|hub| {
            let (item, _) = self.at(&hub, slot)?;
            Some(row::key(&self.key, &item.id))
        });
        // A row that cannot be read keeps a key of its own, so the list still lays out.
        found.unwrap_or_else(|| format!("{}.gone.{slot}", self.key))
    }

    fn row_at(self: &Rc<Self>, win: &Win, slot: usize) -> El {
        let built = win
            .hub
            .try_borrow()
            .ok()
            .and_then(|hub| row::build(self, &hub, slot));
        built.unwrap_or_else(|| el().w_full().h(self.row_height()))
    }
}

/// A number that moves when any token does, so kept rows are drawn again in a new theme.
fn theme_stamp(tokens: &Tokens) -> u64 {
    let mut hasher = DefaultHasher::new();
    let named = tokens.values();
    let named = named.iter().map(|(name, value)| (*name, value));
    let extra = tokens
        .extra
        .iter()
        .map(|(name, value)| (name.as_str(), value));
    for (name, value) in named.chain(extra) {
        name.hash(&mut hasher);
        match value {
            ThemeValue::Flag(flag) => flag.hash(&mut hasher),
            ThemeValue::Number(number) => number.to_bits().hash(&mut hasher),
            ThemeValue::Text(text) => text.hash(&mut hasher),
        }
    }
    hasher.finish()
}

/// A number that moves when a kept row of a feed's table must be drawn again.
fn stamp_of(tokens: &Tokens, desc: &ListDesc, table: &Table, follow: bool, kept: u32) -> u64 {
    let mut hasher = DefaultHasher::new();
    let lines = desc.height == RowHeight::Line;
    let columns = (desc.columns.as_ptr(), desc.columns.len(), desc.tree, lines);
    let opened = (desc.kind, desc.detail, desc.facts_per_line);
    (
        table.version(),
        theme_stamp(tokens),
        columns,
        opened,
        (follow, kept),
    )
        .hash(&mut hasher);
    hasher.finish()
}

/// The column titles, on the same grid as the rows. A list with no titles has no such row.
fn titles(look: &Look, key: &str, desc: &ListDesc, kept: u32) -> Option<El> {
    desc.columns
        .iter()
        .any(|column| !column.title.is_empty())
        .then(|| {
            let title = |column: &Col| {
                let title = words(column.title, TITLE, look.faint, column);
                fit(title.text_case(TextCase::Uppercase), column.width)
            };
            row()
                .key(format!("{key}.table-header"))
                .w_full()
                .h(look.row)
                .px(look.gutter)
                .gap(GAP)
                .center_y()
                .children(shown(desc, kept).map(|(_, column)| column).map(title))
        })
}

/// The columns `kept` names, each with its place among all of them.
pub(super) fn shown(desc: &ListDesc, kept: u32) -> impl Iterator<Item = (usize, &Col)> {
    let all = desc.columns.iter().enumerate();
    all.filter(move |(at, _)| columns::has(kept, *at))
}

/// A plain sentence on a row of its own: a feed's state, or the rule a pane ends with.
fn sentence(look: &Look, said: String) -> El {
    row()
        .w_full()
        .h(look.row)
        .px(look.gutter)
        .center_y()
        .child(text(said).font(META).color(look.faint).truncate())
}

/// What a list says with no words of its pane's own.
pub const WORDS: Words = Words {
    loading: Said {
        what: "Loading",
        next: "",
        button: None,
    },
    empty: Said {
        what: "Nothing here yet",
        next: "",
        button: None,
    },
    offline: Said {
        what: "Offline",
        next: "What was last known shows once it is back.",
        button: None,
    },
    error: Said {
        what: "Could not load",
        next: "",
        button: None,
    },
};

/// The scroll viewport over the rows. It owns the keyboard, so it keeps focus while rows
/// scroll in and out, and it follows its end when asked unless a row is open.
fn rows(frame: Frame, table: &Table) -> El {
    let follow = frame.follow;
    let frame = Rc::new(frame);
    let (keyed, built, typed) = (frame.clone(), frame.clone(), frame.clone());
    let stack = col()
        .key(format!("{}.stack", frame.key))
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            frame.order.len(table),
            frame.row_height(),
            move |win: &Win, slot| Cow::Owned(keyed.key_at(win, slot)),
            move |win: &Win, slot| built.row_at(win, slot),
        );
    let stack = stack.virtual_rows_version((table.keys_version(), frame.desc.tree));
    let viewport = col()
        .key(frame.rows_key())
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: follow && frame.open.is_none(),
        })
        .focusable(true)
        .on_key(move |win: &mut Win, event: &KeyEvent, tx: &mut Tx| {
            keys::key(&typed, win, event, tx)
        })
        .child(stack);
    if follow {
        viewport.on_at_bottom(move |win: &mut Win, at_tail| {
            frame.change(win, |state| state.saw_tail(at_tail));
        })
    } else {
        viewport
    }
}

/// A list: column titles, the feed's state, the rows, the footer. With `follow` it keeps its
/// newest row in view.
fn draw(
    pane: &PaneCx<'_>,
    kind: PaneKind,
    name: &'static str,
    desc: &ListDesc,
    table: Option<&Table>,
    follow: bool,
    words: &Words,
) -> El {
    let tokens = pane.tokens();
    let look = Look::of(tokens);
    let key = match name {
        "" => pane.key(kind),
        name => format!("{}.{name}", pane.key(kind)),
    };
    let fit_key = key.clone();
    let state = pane.state::<Lists>().and_then(|lists| lists.list(name));
    let kept = state
        .and_then(ListState::fit)
        .unwrap_or_else(|| columns::all(desc));
    let head = titles(&look, &key, desc, kept);
    let foot = desc.footer.map(|rule| {
        sentence(&look, rule.to_owned())
            .key(format!("{key}.footer"))
            .border_sides([1.0, 0.0, 0.0, 0.0], look.hairline)
    });
    // A table that is not live with rows shows its state; one that keeps rows says why above them.
    let shown = Status::of_table(table);
    let notice = table.and_then(|table| match shown {
        None => notice(table.status()),
        Some(_) => None,
    });
    let instead = shown.map(|status| states::view(pane, &key, &status, words));
    let body = table.filter(|_| instead.is_none()).map_or_else(
        || instead.unwrap_or_else(|| el().grow(1.0)),
        |table| {
            let source = Source::of(pane, table);
            let stamp = stamp_of(tokens, desc, table, follow, kept);
            // A row that left its table is no longer open: a log goes on once it is trimmed.
            let here = |row: &&RowId| table.find(row).is_some();
            let frame = Frame {
                pane: pane.pane,
                part: name,
                key,
                desc: *desc,
                look,
                source,
                order: Order::of(desc.tree, table),
                follow,
                kept,
                stamp,
                open: state.and_then(ListState::open).filter(here).cloned(),
                cursor: state.and_then(ListState::cursor).cloned(),
                said: state.and_then(ListState::said).map(str::to_owned),
            };
            rows(frame, table)
        },
    );
    let (id, whole, gutter, part) = (pane.pane, *desc, look.gutter, name);
    col()
        .key(format!("{fit_key}.fit"))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .on_layout_tx(move |win: &mut Win, at: [f32; 4], _: &mut Tx| {
            let kept = columns::kept(&whole, at[2] - 2.0 * gutter);
            let Ok(mut hub) = win.hub.try_borrow_mut() else {
                return;
            };
            if let Some(lists) = hub.panes.get_mut::<Lists>(Some(id)) {
                let list = lists.list_mut(part);
                if list.fit().unwrap_or_else(|| columns::all(&whole)) != kept {
                    list.set_fit(kept);
                }
            }
        })
        .children(head)
        .children(notice.map(|said| sentence(&look, said)))
        .child(body)
        .children(foot)
}

/// A list as one part of a pane that draws more than it. `name` keys the list and its open
/// slot (`<pane key>.<name>`); a pane that is one list has the name `""`.
pub fn part(
    pane: &PaneCx<'_>,
    kind: PaneKind,
    name: &'static str,
    desc: &ListDesc,
    table: Option<&Table>,
) -> El {
    draw(pane, kind, name, desc, table, false, &WORDS)
}

/// [`part`] or [`stream`] that says the pane's own words when its table has no rows to show.
pub fn worded(
    pane: &PaneCx<'_>,
    kind: PaneKind,
    name: &'static str,
    desc: &ListDesc,
    table: Option<&Table>,
    words: &Words,
    follow: bool,
) -> El {
    draw(pane, kind, name, desc, table, follow, words)
}

/// [`part`] for rows that keep arriving, as a log's do: it keeps its newest row in view
/// unless the reader scrolled up or a row is open.
pub fn stream(
    pane: &PaneCx<'_>,
    kind: PaneKind,
    name: &'static str,
    desc: &ListDesc,
    table: Option<&Table>,
) -> El {
    draw(pane, kind, name, desc, table, true, &WORDS)
}

/// One list: a header bar, column titles, then the feed's state or its rows.
pub fn list(pane: &PaneCx<'_>, kind: PaneKind, desc: &ListDesc, table: Option<&Table>) -> El {
    let tokens = pane.tokens();
    col()
        .key(pane.key(kind))
        .w_full()
        .h_full()
        .fill(colour(tokens.pane_tint.alpha(tokens.glass_pane)))
        .child(
            header(tokens, vec![lead(tokens, kind.title())], Vec::new())
                .key(format!("{}.pane-header", pane.key(kind))),
        )
        .child(part(pane, kind, "", desc, table))
}

#[cfg(test)]
mod tests;
