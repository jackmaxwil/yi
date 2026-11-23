//! The set of windows: which panes exist, which window each is in, and each window's grid.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::grid::layout::{ChatSide, Edge, GridError, Layout, Line, Point, Rect, Size};
use crate::grid::place::{line_at, place};
use crate::grid::saved::{Own, OwnId, Slug};
use crate::grid::swap::{Spot, Zone, toward, zone_at};
use crate::ids::{PaneId, WindowId};
use crate::pane::PaneKind;

const NOMINAL: Size = Size {
    w: 1200.0,
    h: 750.0,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pane {
    #[serde(with = "crate::grid::layout::store::pane")]
    pub id: PaneId,
    #[serde(with = "crate::grid::layout::store::kind")]
    pub kind: PaneKind,
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
}

impl TryFrom<Unchecked> for Windows {
    type Error = GridError;

    fn try_from(file: Unchecked) -> Result<Self, GridError> {
        let mut grid = Windows {
            side: file.side,
            panes: file.panes,
            windows: file.windows,
            own: Vec::new(),
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

impl Windows {
    #[must_use]
    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|window| window.id == id)
    }

    #[must_use]
    pub fn kind(&self, pane: PaneId) -> Option<PaneKind> {
        let found = self.panes.iter().find(|other| other.id == pane);
        found.map(|pane| pane.kind)
    }

    /// The window `pane` is drawn in.
    #[must_use]
    pub fn window_of(&self, pane: PaneId) -> Option<WindowId> {
        let holds = |window: &&Window| window.layout.docked().any(|(other, _)| other == pane);
        self.windows.iter().find(holds).map(|window| window.id)
    }

    /// The window `pane` was pulled out of, where it docks back.
    #[must_use]
    pub fn origin_of(&self, pane: PaneId) -> Option<WindowId> {
        let left = |window: &&Window| window.layout.is_away(pane);
        self.windows.iter().find(left).map(|window| window.id)
    }

    /// Every pane's rectangle in `window`, for a grid of `size`, chat on the side set.
    pub fn place(&self, window: WindowId, size: Size) -> Vec<(PaneId, Rect)> {
        let found = self.window(window);
        found.map_or_else(Vec::new, |window| place(&window.layout, size, self.side))
    }

    /// The line a pointer at `at` in `window` would drag.
    #[must_use]
    pub fn line_at(&self, window: WindowId, size: Size, at: Point) -> Option<Line> {
        line_at(&self.window(window)?.layout, size, self.side, at)
    }

    /// The drop zone under `at` while `pane` is carried in `window`.
    #[must_use]
    pub fn zone_at(&self, window: WindowId, size: Size, pane: PaneId, at: Point) -> Option<Zone> {
        zone_at(&self.window(window)?.layout, self.side, size, pane, at)
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

    /// Whether every window is a whole grid and every pane is in one place: drawn in one
    /// window at most, and out of a window only while another draws it. A file must pass.
    pub fn check(&self) -> Result<(), GridError> {
        let mut known = BTreeSet::new();
        if let Some(pane) = self.panes.iter().find(|pane| !known.insert(pane.id)) {
            return Err(GridError::AlreadyIn(pane.id));
        }
        let (mut windows, mut drawn, mut out) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
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
            let docked = window.layout.docked().map(|(pane, _)| pane);
            let left = window.layout.away.iter().map(|hole| hole.pane);
            let twice = docked.into_iter().find(|pane| !drawn.insert(*pane));
            if let Some(pane) = twice.or_else(|| left.into_iter().find(|pane| !out.insert(*pane))) {
                return Err(GridError::AlreadyIn(pane));
            }
        }
        match out.difference(&drawn).next() {
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
