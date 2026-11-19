//! What the pane remembers between frames: its filters, the field they were typed in, and the
//! view last made of the lines.

use std::cell::RefCell;
use std::rc::Rc;

use rune::TextEdit;
use yd_core::ids::{Millis, PaneId, RowId};
use yd_ui::pane::PaneCx;
use yd_ui::win::{Win, field};

use super::shown::Shown;

/// The text filter's field in the window that edits it.
const FILTER: &str = "filter";

/// The four levels, in the order the header lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

impl Level {
    pub const ALL: [Level; 4] = [Level::Error, Level::Warn, Level::Info, Level::Debug];

    pub fn title(self) -> &'static str {
        match self {
            Level::Error => "Error",
            Level::Warn => "Warn",
            Level::Info => "Info",
            Level::Debug => "Debug",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Debug => "debug",
        }
    }

    /// The level a line's `level` cell names, whatever its case.
    pub fn of(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|level| level.slug().eq_ignore_ascii_case(word))
    }
}

/// Which lines the list shows. A level that is off hides its lines (record 5.5).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Filters {
    pub levels: [bool; 4],
    pub source: Option<String>,
    /// Only lines within [`super::shown::AROUND`] of this time.
    pub around: Option<Millis>,
    /// Windows of older lines loaded past the newest one.
    pub older: usize,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            levels: [true, true, true, false],
            source: None,
            around: None,
            older: 0,
        }
    }
}

impl Filters {
    pub fn on(&self, level: Level) -> bool {
        self.levels.get(level as usize).copied().unwrap_or(false)
    }

    pub fn flip(&mut self, level: Level) {
        if let Some(on) = self.levels.get_mut(level as usize) {
            *on = !*on;
        }
        self.older = 0;
    }
}

/// The view last made, with what it was made from: everything but the feed's rows, and how
/// far into the feed it read.
pub struct Made {
    pub rest: u64,
    pub fed: usize,
    pub tail: Option<RowId>,
    pub version: u64,
    pub shown: Rc<Shown>,
}

/// The pane's own state; it travels with the pane.
#[derive(Default)]
pub struct State {
    pub filters: Filters,
    /// The text filter as last changed: the window that edits it holds the field itself.
    pub field: TextEdit,
    /// Whether the source has been asked for lines.
    pub asked: bool,
    pub made: RefCell<Option<Made>>,
}

/// Changes this pane's state from a handler.
pub fn change(win: &mut Win, pane: PaneId, edit: impl FnOnce(&mut State)) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(state) = hub.panes.get_mut::<State>(Some(pane))
    {
        edit(state);
    }
}

/// Whether the window's field is the one to trust: it holds the keyboard, or it still says
/// what the hub last heard. Otherwise another window changed the text since.
fn current(own: &TextEdit, kept: Option<&State>) -> bool {
    own.focused || kept.is_none_or(|kept| kept.field.text == own.text)
}

/// The field Rune edits: this window's own, seeded from the hub's.
pub fn edit(win: &mut Win, pane: PaneId) -> &mut TextEdit {
    let Win { hub, edits, .. } = win;
    let hub = hub.try_borrow().ok();
    let kept = hub
        .as_ref()
        .and_then(|hub| hub.panes.get::<State>(Some(pane)));
    let own = edits.entry(field(pane, FILTER)).or_default();
    if !current(own, kept) {
        *own = kept.map(|kept| kept.field.clone()).unwrap_or_default();
    }
    own
}

/// The field to draw. Invariant: Rune keeps focus and caret in the edit the view draws, so
/// this is the one `on_edit` returns whenever the window has it.
pub fn drawn<'a>(pane: &PaneCx<'a>) -> Option<&'a TextEdit> {
    let kept = pane.state::<State>();
    let own = pane
        .edits
        .and_then(|edits| edits.get(&field(pane.pane, FILTER)));
    own.filter(|own| current(own, kept))
        .or(kept.map(|kept| &kept.field))
}

/// Copies the window's field to the hub after each change, so it travels with the pane.
pub fn sync(win: &mut Win, pane: PaneId) {
    let field = edit(win, pane).clone();
    change(win, pane, |state| {
        state.field = field;
        state.filters.older = 0;
    });
}
