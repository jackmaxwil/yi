//! A layout as data: column and row tracks, each pane's slot between grid lines, and the
//! panes that are out of the grid.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::grid::leave::close_up;
use crate::ids::{PaneId, WindowId};

pub(crate) mod store;

/// The smallest a pane may be dragged to, and the smallest window.
pub const MIN_PANE: Size = Size { w: 160.0, h: 120.0 };
pub const MIN_WINDOW: Size = Size { w: 720.0, h: 480.0 };
/// The most tracks an axis holds, and the most panes a grid has slots for. A split that
/// needs one more is refused; the bounds also keep a file from costing more than it should.
pub const MAX_TRACKS: usize = 32;
pub const MAX_PANES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

/// A pane's place inside its grid, in points from the grid's top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// A place in a grid as it is drawn, in points from its top left.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// One row or column: fixed points, or a share of what the fixed ones leave.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Track {
    Px(f32),
    Fr(f32),
}

impl Track {
    /// Its points or its share, whichever it counts in.
    pub(crate) fn size(self) -> f32 {
        match self {
            Track::Px(size) | Track::Fr(size) => size,
        }
    }

    /// The same kind of track at another size.
    pub(crate) fn sized(self, size: f32) -> Track {
        match self {
            Track::Px(_) => Track::Px(size),
            Track::Fr(_) => Track::Fr(size),
        }
    }
}

/// A pane's grid lines, 1-based with exclusive ends, in CSS `grid-area` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "[u16; 4]", into = "[u16; 4]")]
pub struct Area {
    pub r1: u16,
    pub c1: u16,
    pub r2: u16,
    pub c2: u16,
}

pub const fn area(r1: u16, c1: u16, r2: u16, c2: u16) -> Area {
    Area { r1, c1, r2, c2 }
}

impl From<[u16; 4]> for Area {
    fn from([r1, c1, r2, c2]: [u16; 4]) -> Self {
        Area { r1, c1, r2, c2 }
    }
}

impl From<Area> for [u16; 4] {
    fn from(area: Area) -> Self {
        [area.r1, area.c1, area.r2, area.c2]
    }
}

impl Area {
    /// The lines it starts and ends on along `axis`.
    pub fn span(self, axis: Axis) -> (u16, u16) {
        match axis {
            Axis::Cols => (self.c1, self.c2),
            Axis::Rows => (self.r1, self.r2),
        }
    }

    /// The same area between other lines along `axis`.
    pub(crate) fn with(self, axis: Axis, (start, end): (u16, u16)) -> Area {
        match axis {
            Axis::Cols => Area {
                c1: start,
                c2: end,
                ..self
            },
            Axis::Rows => Area {
                r1: start,
                r2: end,
                ..self
            },
        }
    }

    /// Whether `other` lies wholly inside it.
    pub(crate) fn holds(self, other: Area) -> bool {
        [Axis::Cols, Axis::Rows].into_iter().all(|axis| {
            let ((start, end), (from, to)) = (self.span(axis), other.span(axis));
            start <= from && to <= end
        })
    }
}

/// Saved layouts are written for chat on the right; the left mirrors every column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatSide {
    Right,
    #[default]
    Left,
}

impl ChatSide {
    pub fn flipped(self) -> Self {
        match self {
            ChatSide::Right => ChatSide::Left,
            ChatSide::Left => ChatSide::Right,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Cols,
    Rows,
}

impl Axis {
    pub fn other(self) -> Axis {
        match self {
            Axis::Cols => Axis::Rows,
            Axis::Rows => Axis::Cols,
        }
    }
}

/// A side of a pane, in the layout's own columns: chat on the left swaps what is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

impl Edge {
    /// The order a gap looks for panes to fill it: above and below before the sides.
    pub(crate) const ALL: [Edge; 4] = [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right];

    /// The axis it lies across, and whether it is the first end of that axis.
    pub(crate) fn parts(self) -> (Axis, bool) {
        match self {
            Edge::Top => (Axis::Rows, true),
            Edge::Bottom => (Axis::Rows, false),
            Edge::Left => (Axis::Cols, true),
            Edge::Right => (Axis::Cols, false),
        }
    }

    pub(crate) fn opposite(self) -> Edge {
        match self {
            Edge::Top => Edge::Bottom,
            Edge::Bottom => Edge::Top,
            Edge::Left => Edge::Right,
            Edge::Right => Edge::Left,
        }
    }
}

/// The line between track `after` (0-based) and the next one on `axis`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    pub axis: Axis,
    pub after: u16,
}

/// A pane that is out of the grid. Its slot stays in the layout, closed up when drawn; the
/// neighbours it remembers place it in a layout that has no slot for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hole {
    #[serde(with = "store::pane")]
    pub pane: PaneId,
    /// Each neighbour it had, with the edge of that neighbour it sat on.
    #[serde(default, with = "store::keyed")]
    pub beside: Vec<(PaneId, Edge)>,
}

/// One window's grid: tracks, and a slot for every pane that is in it or out of it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Layout {
    pub cols: Vec<Track>,
    pub rows: Vec<Track>,
    /// Every slot in layout order: the panes that are in, and the kept slots of those out.
    #[serde(with = "store::keyed")]
    pub areas: Vec<(PaneId, Area)>,
    /// The panes that are out, in the order they left.
    #[serde(default)]
    pub away: Vec<Hole>,
}

impl Layout {
    /// One pane filling the grid: what a pulled-out pane's window shows.
    pub fn single(pane: PaneId) -> Layout {
        Layout {
            cols: vec![Track::Fr(1.0)],
            rows: vec![Track::Fr(1.0)],
            areas: vec![(pane, area(1, 1, 2, 2))],
            away: Vec::new(),
        }
    }

    /// The slot `pane` has here, whether it is in or out.
    #[must_use]
    pub fn slot(&self, pane: PaneId) -> Option<Area> {
        let found = self.areas.iter().find(|(other, _)| *other == pane);
        found.map(|(_, area)| *area)
    }

    /// Whether `pane` is out of this grid.
    pub fn is_away(&self, pane: PaneId) -> bool {
        self.away.iter().any(|hole| hole.pane == pane)
    }

    /// The panes that are in, with their slots, in layout order.
    pub fn docked(&self) -> impl Iterator<Item = (PaneId, Area)> {
        let slots = self.areas.iter().copied();
        slots.filter(|(pane, _)| !self.is_away(*pane))
    }

    /// Whether this is a grid every change can keep whole: sound tracks, slots that tile it
    /// and cut apart, no pane twice, and at least one pane in. A layout from a file must pass.
    pub fn check(&self) -> Result<(), GridError> {
        // A size of nothing is sound: a track a drag left no room of its own, kept by a pane
        // that spans it; drawn alone it gets the minimum.
        let sound = |list: &[Track]| {
            let sizes = list.iter().map(|track| track.size());
            (1..=MAX_TRACKS).contains(&list.len())
                && sizes
                    .into_iter()
                    .all(|size| size >= 0.0 && size.is_finite())
        };
        let fits = sound(&self.cols) && sound(&self.rows) && self.areas.len() <= MAX_PANES;
        if !fits || !tiles(&self.areas, self.rows.len(), self.cols.len()) {
            return Err(GridError::NotTiled);
        }
        let slots = self.areas.iter().map(|(pane, _)| *pane);
        let out = self.away.iter().map(|hole| hole.pane);
        if let Some(pane) = twice(slots).or_else(|| twice(out)) {
            return Err(GridError::AlreadyIn(pane));
        }
        close_up(self).map(drop)
    }
}

/// The first pane that comes up a second time.
fn twice(mut panes: impl Iterator<Item = PaneId>) -> Option<PaneId> {
    let mut seen = BTreeSet::new();
    panes.find(|pane| !seen.insert(*pane))
}

/// Whether `areas` fill a grid of `rows` by `cols` and come apart one straight cut at a time,
/// edge to edge. A pinwheel fills its grid but cannot be cut, and a gap in one cannot close.
pub(crate) fn tiles(areas: &[(PaneId, Area)], rows: usize, cols: usize) -> bool {
    let (Ok(rows), Ok(cols)) = (u16::try_from(rows), u16::try_from(cols)) else {
        return false;
    };
    let mut pieces = vec![area(1, 1, rows.saturating_add(1), cols.saturating_add(1))];
    let mut whole = 0usize;
    while let Some(piece) = pieces.pop() {
        let inside = || {
            areas
                .iter()
                .map(|(_, area)| *area)
                .filter(|area| piece.holds(*area))
        };
        match (inside().next(), inside().nth(1)) {
            (None, _) => return false,
            (Some(only), None) if only == piece => {
                whole = whole.saturating_add(1);
                continue;
            }
            (Some(_), None) => return false,
            (Some(_), Some(_)) => {}
        }
        let cut = [Axis::Rows, Axis::Cols].into_iter().find_map(|axis| {
            let (start, end) = piece.span(axis);
            let clear = |line: &u16| {
                inside().all(|area| area.span(axis).1 <= *line || area.span(axis).0 >= *line)
            };
            let line = (start.saturating_add(1)..end).find(clear)?;
            Some((
                piece.with(axis, (start, line)),
                piece.with(axis, (line, end)),
            ))
        });
        let Some((first, second)) = cut else {
            return false;
        };
        pieces.extend([first, second]);
    }
    whole == areas.len()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum GridError {
    #[error("pane {0:?} is not where this change needs it")]
    NoSuchPane(PaneId),
    #[error("the layout has no such line between two panes")]
    NoSuchLine,
    #[error("there is no window {0:?}")]
    NoSuchWindow(WindowId),
    #[error("there is no saved layout of that name")]
    NoSuchLayout,
    #[error("pane {0:?} is the last one in its window")]
    LastPane(PaneId),
    #[error("pane {0:?} is already in a layout")]
    AlreadyIn(PaneId),
    #[error("pane {0:?} would be left in no window")]
    Stranded(PaneId),
    #[error("window {0:?} is listed twice")]
    SameWindow(WindowId),
    #[error(
        "the grid is full: it holds {} panes and {} tracks an axis",
        MAX_PANES,
        MAX_TRACKS
    )]
    Full,
    #[error("the layout's panes do not tile its grid")]
    NotTiled,
    #[error("pane {0:?} would be drawn under the pane minimum")]
    Cramped(PaneId),
    #[error("a layout's name is 1 to {} characters", crate::grid::saved::MAX_NAME)]
    NoName,
    #[error(
        "{} layouts of your own is the most there can be",
        crate::grid::saved::MAX_OWN
    )]
    TooManyLayouts,
}
