//! Dropping a carried pane: a middle swaps the two or takes a tab, an edge of a pane gives it
//! half that slot, an edge of the window a column or row. The tabs behind it keep its slot.

use crate::grid::layout::{
    Axis, ChatSide, Edge, GridError, Layout, MAX_PANES, MAX_TRACKS, MIN_PANE, Point, Rect, Size,
    Track, area,
};
use crate::grid::leave::{front, next_tab, part, split, stack};
use crate::grid::place::place;
use crate::ids::PaneId;

/// A pane's edge zone is this share of its width or height from that edge.
pub const BAND: f32 = 0.25;
/// The window's own edge zones are this many points deep, and win over a pane's.
pub const SIDE: f32 = 12.0;
/// A new column along the window's side is Review's chat column wide; a new row is as tall
/// as Review's terminal.
pub const DOCKED: Size = Size { w: 300.0, h: 150.0 };

/// Where a carried pane lands, its edges as drawn: chat on the left mirrors them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spot {
    /// The middle of another pane: the two exchange slots, or a tab joins that pane's tabs.
    Onto(PaneId),
    /// An edge of another pane: the carried one takes the half of its slot on that edge. A
    /// tab's own slot is cut the same way, between it and the tab that takes over.
    Beside(PaneId, Edge),
    /// An edge of the window: a full-height column or full-width row there.
    Side(Edge),
}

/// What lies under the pointer while a pane is carried, as drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Zone {
    pub spot: Spot,
    /// The zone itself: a pane's edge band or middle, or the window's edge band.
    pub band: Rect,
    /// Where the pane would be drawn once dropped; none when the drop is refused.
    pub lands: Option<Rect>,
}

/// Exchanges the slots of two panes that are both in the grid.
pub fn swap(layout: &mut Layout, a: PaneId, b: PaneId) -> Result<(), GridError> {
    let slot = |pane: PaneId| {
        let found = layout.docked().find(|(other, _)| *other == pane);
        found
            .map(|(_, area)| area)
            .ok_or(GridError::NoSuchPane(pane))
    };
    let (of_a, of_b) = (slot(a)?, slot(b)?);
    for (pane, area) in layout.areas.iter_mut() {
        if *pane == a {
            *area = of_b;
        } else if *pane == b {
            *area = of_a;
        }
    }
    Ok(())
}

/// The edge in the layout's own columns that is drawn as `edge`.
pub(crate) fn own(edge: Edge, side: ChatSide) -> Edge {
    match (side, edge) {
        (ChatSide::Left, Edge::Left | Edge::Right) => edge.opposite(),
        (ChatSide::Left, Edge::Top | Edge::Bottom) | (ChatSide::Right, _) => edge,
    }
}

/// Drops `pane` at `spot` in a grid drawn at `size`, alone: the tabs behind it keep its slot,
/// else no hole is left. Refused when a pane would be drawn under the minimum.
pub fn land(
    layout: &mut Layout,
    side: ChatSide,
    pane: PaneId,
    spot: Spot,
    size: Size,
) -> Result<(), GridError> {
    let docked = |layout: &Layout, other: PaneId| layout.docked().any(|(id, _)| id == other);
    if !docked(layout, pane) {
        return Err(GridError::NoSuchPane(pane));
    }
    let mate = next_tab(layout, pane);
    match spot {
        Spot::Onto(other) if other == pane => {}
        Spot::Onto(other) if mate.is_none() => swap(layout, pane, other)?,
        Spot::Onto(other) => join(layout, pane, other)?,
        Spot::Beside(other, edge) => {
            let other = if other == pane { mate } else { Some(other) };
            let other = other.ok_or(GridError::NoSuchPane(pane))?;
            part(layout, pane)?;
            if !docked(layout, other) {
                return Err(GridError::NoSuchPane(other));
            }
            split(layout, other, own(edge, side), pane)?;
        }
        Spot::Side(edge) => {
            part(layout, pane)?;
            along(layout, own(edge, side), pane)?;
        }
    }
    layout.check()?;
    let small = |rect: &Rect| rect.w < MIN_PANE.w - 0.01 || rect.h < MIN_PANE.h - 0.01;
    match place(layout, size, side)
        .iter()
        .find(|(_, rect)| small(rect))
    {
        Some((cramped, _)) => Err(GridError::Cramped(*cramped)),
        None => Ok(()),
    }
}

/// Makes `pane` the tab in front of `other`'s slot. The slot it had goes to the tab behind
/// it, else closes up.
pub fn join(layout: &mut Layout, pane: PaneId, other: PaneId) -> Result<(), GridError> {
    part(layout, pane)?;
    stack(layout, other, pane)?;
    front(layout, pane)
}

/// Puts `pane` in a new track on `edge` of the grid, across every track of the other axis.
fn along(layout: &mut Layout, edge: Edge, pane: PaneId) -> Result<(), GridError> {
    let (axis, first) = edge.parts();
    let (list, across, room) = match axis {
        Axis::Cols => (&mut layout.cols, layout.rows.len(), DOCKED.w),
        Axis::Rows => (&mut layout.rows, layout.cols.len(), DOCKED.h),
    };
    if list.len() >= MAX_TRACKS || layout.areas.len() >= MAX_PANES {
        return Err(GridError::Full);
    }
    let at = if first { 0 } else { list.len() };
    list.insert(at, Track::Px(room));
    let line = u16::try_from(at)
        .map_err(|_| GridError::Full)?
        .saturating_add(1);
    if first {
        for (_, slot) in layout.areas.iter_mut() {
            let (from, to) = slot.span(axis);
            *slot = slot.with(axis, (from.saturating_add(1), to.saturating_add(1)));
        }
    }
    let across = u16::try_from(across).map_err(|_| GridError::Full)?;
    let slot = area(1, 1, 2, 2).with(axis.other(), (1, across.saturating_add(1)));
    layout
        .areas
        .push((pane, slot.with(axis, (line, line.saturating_add(1)))));
    Ok(())
}

fn inside(rect: &Rect, at: Point) -> bool {
    at.x >= rect.x && at.x < rect.x + rect.w && at.y >= rect.y && at.y < rect.y + rect.h
}

/// The spot under `at` in a grid of `size`, with its zone: the window's edge within [`SIDE`]
/// of it, else the band or the middle of the pane drawn there. None outside the grid.
#[must_use]
pub fn spot_at(layout: &Layout, side: ChatSide, size: Size, at: Point) -> Option<(Spot, Rect)> {
    let grid = Rect {
        x: 0.0,
        y: 0.0,
        w: size.w,
        h: size.h,
    };
    if !inside(&grid, at) {
        return None;
    }
    let sides = [
        (Edge::Left, at.x, Rect { w: SIDE, ..grid }),
        (
            Edge::Right,
            size.w - at.x,
            Rect {
                x: size.w - SIDE,
                w: SIDE,
                ..grid
            },
        ),
        (Edge::Top, at.y, Rect { h: SIDE, ..grid }),
        (
            Edge::Bottom,
            size.h - at.y,
            Rect {
                y: size.h - SIDE,
                h: SIDE,
                ..grid
            },
        ),
    ];
    let nearest = sides.into_iter().min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((edge, _, band)) = nearest.filter(|(_, depth, _)| *depth < SIDE) {
        return Some((Spot::Side(edge), band));
    }
    let placed = place(layout, size, side);
    let (other, rect) = placed.iter().find(|(_, rect)| inside(rect, at))?;
    Some(over(*other, *rect, at))
}

/// The zone under `at` while `pane` is carried in a grid of `size`; none outside the grid or
/// where the drop would change nothing: over its own slot, but for an edge a tab can take.
#[must_use]
pub fn zone_at(
    layout: &Layout,
    side: ChatSide,
    size: Size,
    pane: PaneId,
    at: Point,
) -> Option<Zone> {
    let (spot, band) = spot_at(layout, side, size, at)?;
    let own = matches!(spot, Spot::Onto(other) | Spot::Beside(other, _) if other == pane);
    if own && next_tab(layout, pane).is_none() {
        return None;
    }
    let mut next = layout.clone();
    let lands = land(&mut next, side, pane, spot, size)
        .ok()
        .and_then(|()| rect_in(&next, side, size, pane));
    (next != *layout || lands.is_none()).then_some(Zone { spot, band, lands })
}

/// Where `pane` is drawn in a grid of `size` with `layout`, if it is drawn.
pub fn rect_in(layout: &Layout, side: ChatSide, size: Size, pane: PaneId) -> Option<Rect> {
    place(layout, size, side)
        .into_iter()
        .find(|(id, _)| *id == pane)
        .map(|(_, rect)| rect)
}

/// The zone of `other`, drawn at `rect`, that `at` is in: the edge band it is deepest in,
/// measured as a share of the pane's size, else the middle.
fn over(other: PaneId, rect: Rect, at: Point) -> (Spot, Rect) {
    let (across, down) = ((at.x - rect.x) / rect.w, (at.y - rect.y) / rect.h);
    let (wide, tall) = (rect.w * BAND, rect.h * BAND);
    let edges = [
        (Edge::Left, across, Rect { w: wide, ..rect }),
        (
            Edge::Right,
            1.0 - across,
            Rect {
                x: rect.x + rect.w - wide,
                w: wide,
                ..rect
            },
        ),
        (Edge::Top, down, Rect { h: tall, ..rect }),
        (
            Edge::Bottom,
            1.0 - down,
            Rect {
                y: rect.y + rect.h - tall,
                h: tall,
                ..rect
            },
        ),
    ];
    let nearest = edges.into_iter().min_by(|a, b| a.1.total_cmp(&b.1));
    match nearest.filter(|(_, share, _)| *share < BAND) {
        Some((edge, _, band)) => (Spot::Beside(other, edge), band),
        None => (Spot::Onto(other), rect),
    }
}

/// Where a key moves `pane` toward `edge`: onto that edge of the pane drawn just past it,
/// else along that edge of the window.
#[must_use]
pub fn toward(layout: &Layout, side: ChatSide, size: Size, pane: PaneId, edge: Edge) -> Spot {
    let placed = place(layout, size, side);
    let home = placed
        .iter()
        .find(|(id, _)| *id == pane)
        .map(|(_, rect)| *rect);
    let past = home.map(|r| match edge {
        Edge::Left => Point {
            x: r.x - 1.0,
            y: r.y + r.h / 2.0,
        },
        Edge::Right => Point {
            x: r.x + r.w + 1.0,
            y: r.y + r.h / 2.0,
        },
        Edge::Top => Point {
            x: r.x + r.w / 2.0,
            y: r.y - 1.0,
        },
        Edge::Bottom => Point {
            x: r.x + r.w / 2.0,
            y: r.y + r.h + 1.0,
        },
    });
    let next = past.and_then(|at| placed.iter().find(|(_, rect)| inside(rect, at)));
    next.map_or(Spot::Side(edge), |(other, _)| Spot::Beside(*other, edge))
}

#[cfg(test)]
mod tests;
