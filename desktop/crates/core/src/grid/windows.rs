//! The set of windows: which panes exist, which window each is in, and each window's grid.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::address::uri::Address;
use crate::grid::layout::{ChatSide, Edge, GridError, Layout, Line, Point, Rect, Size};
use crate::grid::place::{line_at, place};
use crate::grid::saved::{Own, OwnId, Slug};
use crate::grid::swap::{Spot, Zone, toward, zone_at};
use crate::ids::{PaneId, ThreadId, WindowId};
use crate::pane::{Opens, PaneKind, Place};

const NOMINAL: Size = Size {
    w: 1200.0,
    h: 750.0,
};

/// The most sessions a pane remembers the person opened it in; the oldest is forgotten first.
pub const MAX_SESSIONS: usize = 64;

/// Where a pane the person has shows: in every session of its window, or only in the
/// sessions they opened it in.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    #[default]
    Window,
    Sessions(Vec<ThreadId>),
}

impl Scope {
    pub fn is_window(&self) -> bool {
        *self == Scope::Window
    }

    /// Whether a window showing `thread` shows the pane.
    pub fn holds(&self, thread: Option<&ThreadId>) -> bool {
        match self {
            Scope::Window => true,
            Scope::Sessions(list) => thread.is_some_and(|thread| list.contains(thread)),
        }
    }

    /// A new pane's scope: the session shown, or the window for a kind that is not a
    /// session's and where no session is shown.
    pub(crate) fn new(kind: PaneKind, thread: Option<ThreadId>) -> Scope {
        match thread {
            Some(thread) if !kind.wide() => Scope::Sessions(vec![thread]),
            _ => Scope::Window,
        }
    }

    /// Opens the pane in `thread` too, forgetting the oldest session past the most kept.
    pub(crate) fn open(&mut self, thread: ThreadId) {
        let Scope::Sessions(list) = self else {
            return;
        };
        if !list.contains(&thread) {
            list.push(thread);
        }
        let over = list.len().saturating_sub(MAX_SESSIONS);
        list.drain(..over);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pane {
    #[serde(with = "crate::grid::layout::store::pane")]
    pub id: PaneId,
    #[serde(with = "crate::grid::layout::store::kind")]
    pub kind: PaneKind,
    /// Whether a layout seated it and the person has not asked for it since: it is out
    /// of the grid while its kind has nothing to show.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub seated: bool,
    /// The sessions it shows in once it is the person's; a seated pane follows its kind.
    #[serde(default, skip_serializing_if = "Scope::is_window")]
    pub scope: Scope,
    /// The one thing it is about, where its kind may show many: a child agent for a chat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(with = "crate::grid::layout::store::thing")]
    pub thing: Option<Address>,
    /// A rule opened it as a tab behind another and it has not been drawn since: its tab
    /// carries a dot. Not kept in the file.
    #[serde(skip)]
    pub unseen: bool,
}

impl Pane {
    /// Where it opens: its kind's place, but a chat about one child agent is work.
    #[must_use]
    pub fn place(&self) -> Place {
        self.kind.place_about(self.thing.is_some())
    }
}

/// Where a window sits on the desktop, in points; it may be on any screen.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Frame {
    /// Whether it is a place a window can be: real numbers and some size.
    pub(crate) fn fits(&self) -> bool {
        let real = [self.x, self.y, self.w, self.h]
            .iter()
            .all(|value| value.is_finite());
        real && self.w > 0.0 && self.h > 0.0
    }
}

/// One window: a grid of one or more panes. A pulled-out pane is a window of one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Window {
    #[serde(with = "crate::grid::layout::store::window")]
    pub id: WindowId,
    pub layout: Layout,
    /// The saved layout it shows, by slug, until the user rearranges it.
    #[serde(default, with = "crate::grid::layout::store::slug")]
    pub saved: Option<Slug>,
    /// The layout of the person's own it shows, until they rearrange it.
    #[serde(default)]
    pub own: Option<OwnId>,
    /// Where the real window was last seen, so it can open there again.
    #[serde(default)]
    pub frame: Option<Frame>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(try_from = "Unchecked")]
pub struct Windows {
    /// Incident: every file before chat moved left by default wrote the old default as
    /// `side`; read under a new name, it is dropped once and a choice made since is kept.
    #[serde(rename = "chat")]
    pub side: ChatSide,
    pub panes: Vec<Pane>,
    pub windows: Vec<Window>,
    /// The layouts the person arranged and named, kept with the grid.
    #[serde(default)]
    pub own: Vec<Own>,
    /// The kinds with something to show in each session of a window, as last told; none
    /// before its first telling, when no pane rests or comes back.
    #[serde(default, with = "have")]
    pub have: BTreeMap<(WindowId, Option<ThreadId>), BTreeSet<PaneKind>>,
    /// The session each window shows, as last told; none while it shows New chat.
    #[serde(skip)]
    pub shows: BTreeMap<WindowId, ThreadId>,
    /// The tab in front of each stack when a window last showed a session, so it is in
    /// front again when the window turns back. Not kept in the file.
    #[serde(skip)]
    pub fronts: BTreeMap<(WindowId, Option<ThreadId>), Vec<PaneId>>,
    /// The person's word on kinds opening by themselves, as the settings say it. Not kept.
    #[serde(skip)]
    pub rules: BTreeMap<PaneKind, Opens>,
    /// The pane that has each window's whole grid for now, the others out of sight. Not kept.
    #[serde(skip)]
    pub filled: BTreeMap<WindowId, PaneId>,
}

/// A grid as a file gives it, before anything has looked at it.
#[derive(Deserialize)]
struct Unchecked {
    #[serde(default, rename = "chat")]
    side: ChatSide,
    #[serde(default)]
    panes: Vec<Pane>,
    #[serde(default)]
    windows: Vec<Window>,
    #[serde(default)]
    own: Vec<Own>,
    #[serde(default, with = "have")]
    have: BTreeMap<(WindowId, Option<ThreadId>), BTreeSet<PaneKind>>,
}

impl TryFrom<Unchecked> for Windows {
    type Error = GridError;

    fn try_from(file: Unchecked) -> Result<Self, GridError> {
        let mut grid = Windows {
            side: file.side,
            panes: file.panes,
            windows: file.windows,
            own: Vec::new(),
            have: file.have,
            shows: BTreeMap::new(),
            fronts: BTreeMap::new(),
            rules: BTreeMap::new(),
            filled: BTreeMap::new(),
        };
        grid.check()?;
        // A kept layout a file spoils reads as gone, as a saved one this build lacks does.
        for own in file.own.into_iter().filter(Own::fits) {
            if !grid.own.iter().any(|kept| kept.id == own.id) {
                grid.own.push(own);
            }
        }
        let kept: Vec<OwnId> = grid.own.iter().map(|own| own.id).collect();
        for window in &mut grid.windows {
            window.frame = window.frame.filter(Frame::fits);
            window.own = window.own.filter(|id| kept.contains(id));
        }
        Ok(grid)
    }
}

mod have {
    use std::collections::{BTreeMap, BTreeSet};

    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::grid::layout::store::kind::read;
    use crate::ids::{ThreadId, WindowId};
    use crate::pane::PaneKind;

    type Told = BTreeMap<(WindowId, Option<ThreadId>), BTreeSet<PaneKind>>;

    pub fn serialize<S: Serializer>(have: &Told, to: S) -> Result<S::Ok, S::Error> {
        let names =
            |kinds: &BTreeSet<PaneKind>| kinds.iter().map(|kind| kind.slug()).collect::<Vec<_>>();
        to.collect_seq(
            have.iter()
                .map(|((window, thread), kinds)| (window.0, thread, names(kinds))),
        )
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<Told, D::Error> {
        let entries = Vec::<(u32, Option<ThreadId>, Vec<String>)>::deserialize(from)?;
        let mut have = BTreeMap::new();
        for (window, thread, names) in entries {
            let kinds = names
                .iter()
                .map(|name| read::<D::Error>(name))
                .collect::<Result<BTreeSet<_>, _>>()?;
            let key = (WindowId(window), thread);
            if have.insert(key, kinds).is_some() {
                return Err(D::Error::custom("a window session told twice"));
            }
        }
        Ok(have)
    }
}

impl Windows {
    #[must_use]
    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|window| window.id == id)
    }

    fn pane_of(&self, pane: PaneId) -> Option<&Pane> {
        self.panes.iter().find(|other| other.id == pane)
    }

    fn window_where(&self, holds: impl Fn(&Layout) -> bool) -> Option<WindowId> {
        self.windows
            .iter()
            .find(|window| holds(&window.layout))
            .map(|window| window.id)
    }

    #[must_use]
    pub fn kind(&self, pane: PaneId) -> Option<PaneKind> {
        self.pane_of(pane).map(|pane| pane.kind)
    }

    /// Where `pane` goes: the place the person always opens its kind in, else its own.
    #[must_use]
    pub fn place_of(&self, pane: &Pane) -> Place {
        match self.rules.get(&pane.kind) {
            Some(Opens::Always(place)) => *place,
            _ => pane.place(),
        }
    }

    /// The place of the slot `pane` is in: where the first pane of its stack goes.
    #[must_use]
    pub fn slot_place(&self, pane: PaneId) -> Option<Place> {
        let window = self.window_of(pane).or_else(|| self.home_of(pane))?;
        let stack = self.window(window)?.layout.stack(pane);
        let first = stack.first().copied().unwrap_or(pane);
        let first = self.panes.iter().find(|other| other.id == first)?;
        Some(self.place_of(first))
    }

    /// The kind of `pane` when a word on opening by itself can cover it: not the chat, nor
    /// a pane about one thing.
    #[must_use]
    pub fn ruled(&self, pane: PaneId) -> Option<PaneKind> {
        let found = self.panes.iter().find(|other| other.id == pane)?;
        (found.kind != PaneKind::Chat && found.thing.is_none()).then_some(found.kind)
    }

    /// Whether a pane of `kind` opens by itself: the person's word, else the kind's rule.
    #[must_use]
    pub fn opens_alone(&self, kind: PaneKind) -> bool {
        match self.rules.get(&kind) {
            Some(Opens::Never) => false,
            Some(Opens::Always(_)) => true,
            None => kind.arrives(),
        }
    }

    /// The thing `pane` is about, when it is about one.
    #[must_use]
    pub fn thing(&self, pane: PaneId) -> Option<&Address> {
        self.pane_of(pane).and_then(|pane| pane.thing.as_ref())
    }

    /// The window `pane` is drawn in.
    #[must_use]
    pub fn window_of(&self, pane: PaneId) -> Option<WindowId> {
        self.window_where(|layout| layout.docked().any(|(other, _)| other == pane))
    }

    /// The window `pane` belongs to and is not pulled out of: where it is drawn, rests, or
    /// waits behind another pane as a tab.
    #[must_use]
    pub fn home_of(&self, pane: PaneId) -> Option<WindowId> {
        self.window_where(|layout| layout.holds(pane) && !layout.pulled(pane))
    }

    /// Whether `pane` has something to show in `window` now: a seated pane by its kind (the
    /// chat always), the person's by the session shown. True before the window is told.
    #[must_use]
    pub fn showing(&self, window: WindowId, pane: PaneId) -> bool {
        let thread = self.shows.get(&window).cloned();
        let have = self.have.get(&(window, thread.clone()));
        let (Some(have), Some(pane)) = (have, self.pane_of(pane)) else {
            return true;
        };
        match pane.seated {
            true => pane.kind == PaneKind::Chat || have.contains(&pane.kind),
            false => pane.scope.holds(thread.as_ref()),
        }
    }

    /// The tabs of the slot `pane` holds in `window`: the panes of its stack with something
    /// to show, in tab order; the pane alone when it shares its slot with none.
    #[must_use]
    pub fn tabs(&self, window: WindowId, pane: PaneId) -> Vec<PaneId> {
        let layout = self.window(window).map(|window| &window.layout);
        let stack = layout.map(|layout| layout.stack(pane)).unwrap_or_default();
        let here = |other: &PaneId| layout.is_some_and(|layout| !layout.pulled(*other));
        let shown = stack.iter().copied().filter(here);
        let shown: Vec<PaneId> = shown.filter(|other| self.showing(window, *other)).collect();
        if shown.is_empty() { vec![pane] } else { shown }
    }

    /// The window `pane` was pulled out of, where it docks back.
    #[must_use]
    pub fn origin_of(&self, pane: PaneId) -> Option<WindowId> {
        self.window_where(|layout| layout.pulled(pane))
    }

    /// Every pane's rectangle in `window`, for a grid of `size`, chat on the side set.
    pub fn place(&self, window: WindowId, size: Size) -> Vec<(PaneId, Rect)> {
        let found = self.window(window);
        let placed = found.map_or_else(Vec::new, |window| place(&window.layout, size, self.side));
        let drawn = |pane: &PaneId| placed.iter().any(|(other, _)| other == pane);
        match self.fills(window).filter(drawn) {
            Some(pane) => {
                vec![(pane, size.whole())]
            }
            None => placed,
        }
    }

    /// The pane that has `window`'s whole grid: the one filled, or the tab of its stack in
    /// front. None while that pane does not show, as in a session it is not open in.
    #[must_use]
    pub fn fills(&self, window: WindowId) -> Option<PaneId> {
        let filled = *self.filled.get(&window)?;
        let front = self.window(window)?.layout.front(filled)?;
        Some(front).filter(|pane| self.window_of(*pane) == Some(window))
    }

    /// The line a pointer at `at` in `window` would drag.
    #[must_use]
    pub fn line_at(&self, window: WindowId, size: Size, at: Point) -> Option<Line> {
        let lines = self.fills(window).is_none();
        line_at(&self.window(window)?.layout, size, self.side, at).filter(|_| lines)
    }

    /// The drop zone under `at` while `pane` is carried in `window`.
    #[must_use]
    pub fn zone_at(&self, window: WindowId, size: Size, pane: PaneId, at: Point) -> Option<Zone> {
        // A filled window draws one pane: there is nowhere to drop another.
        let open = self.fills(window).is_none();
        let zone = zone_at(&self.window(window)?.layout, self.side, size, pane, at)?;
        Some(zone).filter(|zone| open && !self.visiting(window, pane, zone.spot))
    }

    /// Whether dropping `pane` at `spot` would make it a tab of a pane that is out of
    /// another window. That pane's tabs are kept where it came from, so it takes none here.
    #[must_use]
    pub fn visiting(&self, window: WindowId, pane: PaneId, spot: Spot) -> bool {
        let tabbed = |window: &Window| !window.layout.stack(pane).is_empty();
        let guest = matches!(spot, Spot::Onto(other) if self.origin_of(other).is_some());
        guest && self.window(window).is_some_and(tabbed)
    }

    /// Where a key moves `pane` toward `edge` of `window`, as the window was last seen.
    #[must_use]
    pub fn toward(&self, window: WindowId, pane: PaneId, edge: Edge) -> Option<Spot> {
        let layout = &self.window(window)?.layout;
        Some(toward(layout, self.side, self.room(window), pane, edge))
    }

    /// The size a window's panes are ranked at: its real frame once known, else the app's
    /// first window.
    #[must_use]
    pub fn room(&self, window: WindowId) -> Size {
        let frame = self.window(window).and_then(|window| window.frame);
        frame.map_or(NOMINAL, |frame| Size {
            w: frame.w,
            h: frame.h,
        })
    }

    /// Whether every window is whole and every pane in one place: drawn in one window at most,
    /// pulled out only while another draws it, resting only while none does. A file must pass.
    pub fn check(&self) -> Result<(), GridError> {
        let mut known = BTreeSet::new();
        if let Some(pane) = self.panes.iter().find(|pane| !known.insert(pane.id)) {
            return Err(GridError::AlreadyIn(pane.id));
        }
        let (mut windows, mut drawn, mut out) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        let mut resting = BTreeSet::new();
        for window in &self.windows {
            if !windows.insert(window.id) {
                return Err(GridError::SameWindow(window.id));
            }
            window.layout.check()?;
            if let Some((pane, _)) = window
                .layout
                .areas
                .iter()
                .find(|(pane, _)| !known.contains(pane))
            {
                return Err(GridError::NoSuchPane(*pane));
            }
            let behind = window.layout.stacks.iter().flatten();
            let behind = behind.filter(|pane| {
                window.layout.front(**pane) != Some(**pane) && !window.layout.is_away(**pane)
            });
            if let Some(pane) = window
                .layout
                .stacks
                .iter()
                .flatten()
                .find(|p| !known.contains(p))
            {
                return Err(GridError::NoSuchPane(*pane));
            }
            // A pane behind another is drawn nowhere, as a resting one, and is in one window.
            for pane in behind {
                if !out.insert(*pane) {
                    return Err(GridError::AlreadyIn(*pane));
                }
                resting.insert(*pane);
            }
            let docked = window.layout.docked().map(|(pane, _)| pane);
            let left = window.layout.away.iter().map(|hole| hole.pane);
            let rests = window.layout.away.iter().filter(|hole| hole.resting);
            resting.extend(rests.map(|hole| hole.pane));
            let twice = docked.into_iter().find(|pane| !drawn.insert(*pane));
            if let Some(pane) = twice.or_else(|| left.into_iter().find(|pane| !out.insert(*pane))) {
                return Err(GridError::AlreadyIn(pane));
            }
        }
        if let Some(pane) = resting.intersection(&drawn).next() {
            return Err(GridError::AlreadyIn(*pane));
        }
        let mut lost = out
            .difference(&drawn)
            .filter(|pane| !resting.contains(pane));
        match lost.next() {
            Some(pane) => Err(GridError::NoSuchPane(*pane)),
            None => Ok(()),
        }
    }
}

/// What the grid asks of the real windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowOp {
    Open(WindowId),
    Close(WindowId),
}
