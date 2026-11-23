//! Opening a pane where panes of its place go: for the person, and by itself for a rule.

use super::{add, edit, find, fresh, handled, idle, raise};
use crate::address::uri::Address;
use crate::grid::layout::{ChatSide, Edge, GridError, Layout, Point, Size};
use crate::grid::leave::{split, stack};
use crate::grid::place::place;
use crate::grid::saved::JUST_CHAT;
use crate::grid::swap::{Spot, Zone, join, land, own, spot_at};
use crate::grid::windows::{Pane, Scope, WindowOp, Windows};
use crate::ids::{PaneId, WindowId};
use crate::pane::{PaneKind, Place};

/// The pane holding the first slot of `layout` whose place is `place`: its stack's first
/// pane, or the pane alone, is of a kind that goes there. A pulled-out pane's slot is none.
fn host(grid: &Windows, layout: &Layout, place: Place) -> Option<PaneId> {
    let first = |held: PaneId| layout.stack(held).first().copied().unwrap_or(held);
    let placed = |held: &PaneId| {
        let here = !layout.pulled(*held) && grid.origin_of(*held).is_none();
        let first = grid.panes.iter().find(|pane| pane.id == first(*held));
        here && first.map(|pane| grid.place_of(pane)) == Some(place)
    };
    layout.areas.iter().map(|(pane, _)| *pane).find(placed)
}

/// Shows a pane of `kind` in `id`, the person's: one the window has comes in front, else one
/// joins its place's stack or, with `cut`, takes that edge of the named or its place's slot.
pub(super) fn open(
    grid: &mut Windows,
    id: WindowId,
    kind: PaneKind,
    thing: Option<Address>,
    cut: Option<(Option<PaneId>, Edge)>,
) -> Result<Vec<WindowOp>, GridError> {
    let layout = &grid.window(id).ok_or(GridError::NoSuchWindow(id))?.layout;
    let of_kind =
        |pane: &PaneId| grid.kind(*pane) == Some(kind) && grid.thing(*pane) == thing.as_ref();
    let docked = layout.docked().map(|(pane, _)| pane).find(of_kind);
    let behind = layout.stacks.iter().flatten().copied();
    let behind = behind.filter(|pane| !layout.pulled(*pane)).find(of_kind);
    let resting = layout.away.iter().filter(|hole| hole.resting);
    let resting = resting.map(|hole| hole.pane).find(of_kind);
    if let Some(pane) = docked.or(behind).or(resting) {
        handled(grid, id, pane);
        raise(grid, id, pane)?;
        return Ok(Vec::new());
    }
    // A thing is shown once: its pane out in a window of its own is where it shows.
    let out = |pane: &&Pane| thing.is_some() && grid.origin_of(pane.id).is_some();
    let same = |pane: &&Pane| pane.kind == kind && pane.thing == thing;
    if grid.panes.iter().filter(same).any(|pane| out(&pane)) {
        return Ok(Vec::new());
    }
    let hidden = grid.panes.iter();
    let hidden = hidden.filter(|pane| pane.kind == kind && pane.thing == thing);
    let hidden = hidden.map(|pane| pane.id).find(|pane| idle(grid, *pane));
    let pane = match hidden {
        Some(pane) => pane,
        None => add(grid, kind)?,
    };
    if let Some(made) = grid.panes.iter_mut().find(|other| other.id == pane) {
        made.thing = thing;
    }
    fresh(grid, id, pane);
    let layout = &grid.window(id).ok_or(GridError::NoSuchWindow(id))?.layout;
    let goes = grid.panes.iter().find(|other| other.id == pane);
    let goes = goes.map(|made| grid.place_of(made));
    let beside = cut.and_then(|(named, edge)| {
        let slot = named.or_else(|| host(grid, layout, goes?))?;
        Some((layout.front(slot).unwrap_or(slot), own(edge, grid.side)))
    });
    match beside {
        Some((beside, edge)) => edit(grid, id, |layout| split(layout, beside, edge, pane))?,
        None if seat(grid, id, pane)? => raise(grid, id, pane)?,
        None => {}
    }
    Ok(Vec::new())
}

/// Shows the pane about `thing` at `spot` in `id`: the one the window has, else a new one,
/// dropped as a pane carried there is. On a pane's middle it joins that pane's tabs.
pub(in crate::grid) fn put(
    grid: &mut Windows,
    id: WindowId,
    kind: PaneKind,
    thing: &Address,
    spot: Spot,
    size: Size,
) -> Result<Vec<WindowOp>, GridError> {
    // A pane out of another window keeps its tabs where it came from: it takes none here.
    if matches!(spot, Spot::Onto(other) if grid.origin_of(other).is_some()) {
        return Err(GridError::NoSuchWindow(id));
    }
    let ops = open(grid, id, kind, Some(thing.clone()), None)?;
    let pane = about(grid, id, kind, thing).ok_or(GridError::NoSuchWindow(id))?;
    let side = grid.side;
    // On a middle it joins those tabs even when it has none of its own: unlike a carried
    // pane, it has no slot to give in exchange.
    edit(grid, id, |layout| match spot {
        Spot::Onto(other) => join(layout, pane, other),
        Spot::Beside(..) | Spot::Side(_) => land(layout, side, pane, spot, size),
    })?;
    Ok(ops)
}

/// The pane of `kind` about `thing` that `id` draws.
fn about(grid: &Windows, id: WindowId, kind: PaneKind, thing: &Address) -> Option<PaneId> {
    let is = |pane: &&Pane| pane.kind == kind && pane.thing.as_ref() == Some(thing);
    let mut found = grid.panes.iter().filter(is).map(|pane| pane.id);
    found.find(|pane| grid.window_of(*pane) == Some(id))
}

impl Windows {
    /// The drop zone under `at` while `thing` is carried from pane `from` over `window`: where
    /// its pane of `kind` would land. None off the grid and over the pane it came from.
    #[must_use]
    pub fn zone_for(
        &self,
        window: WindowId,
        size: Size,
        (kind, thing): (PaneKind, &Address),
        from: PaneId,
        at: Point,
    ) -> Option<Zone> {
        let layout = &self.window(window)?.layout;
        let (spot, band) = spot_at(layout, self.side, size, at)?;
        let own = matches!(spot, Spot::Onto(other) | Spot::Beside(other, _) if other == from);
        // A filled window draws one pane: there is nowhere to drop another.
        if own || self.fills(window).is_some() {
            return None;
        }
        let mut next = self.clone();
        let put = put(&mut next, window, kind, thing, spot, size);
        let lands = put.ok().and_then(|_| {
            let pane = about(&next, window, kind, thing)?;
            let drawn = next.place(window, size);
            drawn
                .into_iter()
                .find(|(id, _)| *id == pane)
                .map(|(_, rect)| rect)
        });
        Some(Zone { spot, band, lands })
    }
}

/// Puts `pane`, which `id` does not hold, where panes of its place go: a tab of that place's
/// stack, behind the one in front, else a slot of its own. True when it is a tab.
pub(super) fn seat(grid: &mut Windows, id: WindowId, pane: PaneId) -> Result<bool, GridError> {
    let layout = &grid.window(id).ok_or(GridError::NoSuchWindow(id))?.layout;
    let found = grid.panes.iter().find(|other| other.id == pane);
    let goes = grid.place_of(found.ok_or(GridError::NoSuchPane(pane))?);
    if let Some(onto) = host(grid, layout, goes) {
        stack(&mut find(grid, id)?.layout, onto, pane)?;
        return Ok(true);
    }
    let size = grid.room(id);
    let placed = place(layout, size, grid.side);
    let largest = placed
        .into_iter()
        .max_by(|(_, a), (_, b)| (a.w * a.h).total_cmp(&(b.w * b.h)));
    let (beside, room) = largest.ok_or(GridError::NoSuchWindow(id))?;
    let edge = if room.w >= room.h {
        Edge::Right
    } else {
        Edge::Bottom
    };
    edit(grid, id, |layout| split(layout, beside, edge, pane))?;
    // The list column goes to the far side from the chat and the shelf along the bottom,
    // where there is room; else the half it was cut stays.
    let far = match grid.side {
        ChatSide::Left => Edge::Right,
        ChatSide::Right => Edge::Left,
    };
    let at = match goes {
        Place::Side => Some(Spot::Side(far)),
        Place::Shelf => Some(Spot::Side(Edge::Bottom)),
        Place::Chat | Place::Work => None,
    };
    let side = grid.side;
    if let Some(at) = at {
        edit(grid, id, |layout| {
            let mut tried = layout.clone();
            if land(&mut tried, side, pane, at, size).is_ok() {
                *layout = tried;
            }
            Ok(())
        })?;
    }
    Ok(false)
}

/// Opens a pane for each of `kinds` that opens by itself. One with no room opens nothing:
/// the change it came with still holds.
pub(super) fn arrivals(grid: &mut Windows, window: WindowId, kinds: Vec<PaneKind>) {
    for kind in kinds {
        let mut tried = grid.clone();
        if grid.opens_alone(kind) && arrive(&mut tried, window, kind).is_ok() {
            *grid = tried;
        }
    }
}

/// A rule's pane: `kind` has something to show in this session for the first time, so a pane
/// of it joins its place as a quiet tab the window's own. One the window holds is enough.
fn arrive(grid: &mut Windows, window: WindowId, kind: PaneKind) -> Result<(), GridError> {
    let found = grid.window(window).ok_or(GridError::NoSuchWindow(window))?;
    // Just chat is a page to read: nothing opens beside it by itself. Nor in a window that
    // is not the session's home: a pane's own window, Settings, a chat that is a guest.
    let home = |pane: &&Pane| {
        let own = pane.kind == PaneKind::Chat && pane.thing.is_none();
        let guest = grid.origin_of(pane.id).is_some_and(|from| from != window);
        own && found.layout.holds(pane.id) && !guest
    };
    if found.saved == Some(JUST_CHAT.slug) || !grid.panes.iter().any(|pane| home(&pane)) {
        return Ok(());
    }
    let shown = grid.shows.get(&window);
    let here = |pane: &&Pane| pane.seated || pane.scope.holds(shown);
    let held = |pane: &&Pane| pane.kind == kind && pane.thing.is_none() && here(pane);
    let mut held = grid.panes.iter().filter(held);
    if let Some(known) = held
        .find(|pane| found.layout.holds(pane.id))
        .map(|pane| pane.id)
    {
        // The window's own pane of the kind is back with something new: a dot if it is behind.
        for pane in grid.panes.iter_mut().filter(|pane| pane.id == known) {
            pane.unseen = pane.seated;
        }
        return Ok(());
    }
    let hidden = grid
        .panes
        .iter()
        .filter(|pane| pane.kind == kind && pane.thing.is_none());
    let hidden = hidden.map(|pane| pane.id).find(|pane| idle(grid, *pane));
    let pane = match hidden {
        Some(pane) => pane,
        None => add(grid, kind)?,
    };
    let behind = seat(grid, window, pane)?;
    if let Some(made) = grid.panes.iter_mut().find(|other| other.id == pane) {
        (made.seated, made.scope, made.unseen) = (true, Scope::Window, behind);
    }
    Ok(())
}
