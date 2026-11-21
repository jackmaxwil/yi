//! Grid changes that are not commands: a line dragged, a pane dropped, a pane pulled out.

use crate::grid::layout::{Axis, ChatSide, Edge, GridError, Hole, Layout, Line, Size};
use std::collections::BTreeSet;

use crate::grid::leave::{close, front, leave, next_tab, rejoin, rest, split, stack, unstack};
use crate::grid::place::place;
use crate::grid::resize::{even, position, resize};
use crate::grid::saved::{FIRST_OWN, MAX_OWN, Own, OwnId, Shape, Slug, find as find_saved, named};
use crate::grid::swap::{Spot, land, swap};
use crate::grid::windows::{Frame, Pane, Scope, Window, WindowOp, Windows};
use crate::ids::{PaneId, ThreadId, WindowId};
use crate::msg::Effect;
use crate::pane::{PaneKind, Place};

#[derive(Debug, Clone, PartialEq)]
pub enum GridMsg {
    /// Show a saved layout, by slug, in a window; a window the grid lacks is made.
    Show {
        window: WindowId,
        saved: &'static str,
    },
    /// Drag a line to `to` points from the left or top of a grid of `size`, as drawn.
    Resize {
        window: WindowId,
        line: Line,
        to: f32,
        size: Size,
    },
    /// Give the panes either side of a line equal room.
    Even {
        window: WindowId,
        line: Line,
        size: Size,
    },
    Swap {
        window: WindowId,
        a: PaneId,
        b: PaneId,
    },
    /// Drop a carried pane at a spot of a grid drawn at `size`: swapped, beside another pane,
    /// or along the window's edge. Refused where a pane would go under the minimum.
    Drop {
        window: WindowId,
        pane: PaneId,
        at: Spot,
        size: Size,
    },
    /// Pull a pane out into a window of its own.
    Leave(PaneId),
    /// Dock a pulled-out pane back into the window it left.
    Rejoin(PaneId),
    /// Put a second pane of the same kind to the right of a pane.
    Split(PaneId),
    /// Close a pane for good; its neighbours fill the room.
    Close(PaneId),
    /// Show a pane of `kind` in a window that draws none: it takes half of the largest pane.
    Open {
        window: WindowId,
        kind: PaneKind,
    },
    /// The session a window shows and the kinds with something to show in it. A pane a
    /// layout seated follows its kind; one the person has follows the sessions it is open in.
    Have {
        window: WindowId,
        kinds: Vec<PaneKind>,
        thread: Option<ThreadId>,
    },
    /// Bring a pane that waits behind another as a tab in front of its stack.
    Front(PaneId),
    /// Show a pane in every session of its window, or only in the one shown now.
    Across {
        pane: PaneId,
        on: bool,
    },
    /// The real window moved or changed size. A frame that is no place is not kept.
    Moved {
        window: WindowId,
        frame: Frame,
    },
    /// Keep the window's arrangement as a layout of the person's own, under the lowest
    /// number free; it then shows that layout.
    Keep {
        window: WindowId,
        name: String,
    },
    /// Show a layout of the person's own in a window.
    Wear {
        window: WindowId,
        own: OwnId,
    },
    Rename {
        own: OwnId,
        name: String,
    },
    /// Forget a layout of the person's own; a window showing it keeps its arrangement.
    Forget(OwnId),
}

/// Applies a change to the grid. One that cannot be made changes nothing and asks nothing.
pub fn step(grid: &mut Windows, change: GridMsg) -> Vec<Effect> {
    // Invariant: a change happens whole or not at all, so it is made on a copy first.
    let mut next = grid.clone();
    let Ok(ops) = apply(&mut next, change) else {
        return Vec::new();
    };
    *grid = next;
    ops.into_iter().map(Effect::Window).collect()
}

fn apply(grid: &mut Windows, change: GridMsg) -> Result<Vec<WindowOp>, GridError> {
    let side = grid.side;
    match change {
        GridMsg::Show { window, saved } => return show(grid, window, saved),
        GridMsg::Wear { window, own } => {
            let found = grid.own.iter().find(|kept| kept.id == own);
            let shape = found.ok_or(GridError::NoSuchLayout)?.shape.clone();
            return wear(grid, window, &shape, (None, Some(own)));
        }
        GridMsg::Keep { window, name } => keep(grid, window, &name)?,
        GridMsg::Rename { own, name } => {
            let kept = grid.own.iter_mut().find(|kept| kept.id == own);
            let kept = kept.ok_or(GridError::NoSuchLayout)?;
            kept.name = named(&name).ok_or(GridError::NoName)?;
        }
        GridMsg::Forget(own) => {
            grid.own.retain(|kept| kept.id != own);
            for window in grid.windows.iter_mut().filter(|w| w.own == Some(own)) {
                window.own = None;
            }
        }
        GridMsg::Have {
            window,
            kinds,
            thread,
        } => {
            let layout = &find(grid, window)?.layout;
            // The tabs in front as the window turns away: back in front when it turns back.
            let fronts = layout
                .stacks
                .iter()
                .filter_map(|stack| layout.front(*stack.first()?));
            let fronts: Vec<PaneId> = fronts.filter(|pane| !layout.is_away(*pane)).collect();
            let was = grid.shows.get(&window).cloned();
            let turned = was != thread;
            if turned {
                grid.fronts.insert((window, was), fronts);
            }
            grid.have.insert(window, kinds.into_iter().collect());
            match thread.clone() {
                Some(thread) => grid.shows.insert(window, thread),
                None => grid.shows.remove(&window),
            };
            let back = grid.fronts.get(&(window, thread)).cloned();
            for pane in back.unwrap_or_default().into_iter().filter(|_| turned) {
                let here = grid.home_of(pane) == Some(window) && grid.showing(window, pane);
                if here {
                    front(&mut find(grid, window)?.layout, pane)?;
                }
            }
            settle(grid, window)?;
        }
        GridMsg::Front(pane) => {
            let window = grid.home_of(pane).ok_or(GridError::NoSuchPane(pane))?;
            handled(grid, window, pane);
            raise(grid, window, pane)?;
        }
        GridMsg::Across { pane, on } => {
            let window = grid.window_of(pane).ok_or(GridError::NoSuchPane(pane))?;
            let thread = grid.shows.get(&window).cloned().filter(|_| !on);
            let found = grid.panes.iter_mut().find(|other| other.id == pane);
            let found = found.ok_or(GridError::NoSuchPane(pane))?;
            found.seated = false;
            found.scope = thread.map_or(Scope::Window, |thread| Scope::Sessions(vec![thread]));
        }
        GridMsg::Leave(pane) => return pull(grid, pane),
        GridMsg::Rejoin(pane) => return dock(grid, pane),
        GridMsg::Close(pane) => return shut(grid, pane),
        GridMsg::Split(pane) => {
            let (kind, from) = grid
                .kind(pane)
                .zip(grid.window_of(pane))
                .ok_or(GridError::NoSuchPane(pane))?;
            let copy = add(grid, kind)?;
            fresh(grid, from, copy);
            let edge = match side {
                ChatSide::Right => Edge::Right,
                ChatSide::Left => Edge::Left,
            };
            edit(grid, from, |layout| split(layout, pane, edge, copy))?;
        }
        GridMsg::Open { window, kind } => return open(grid, window, kind),
        GridMsg::Resize {
            window,
            line,
            to,
            size,
        } => {
            let mirrored = line.axis == Axis::Cols && side == ChatSide::Left;
            let to = if mirrored { size.w - to } else { to };
            edit(grid, window, |layout| {
                let by = to - position(layout, line, size)?;
                resize(layout, line, by, size)
            })?;
        }
        GridMsg::Even { window, line, size } => {
            edit(grid, window, |layout| even(layout, line, size))?
        }
        GridMsg::Swap { window, a, b } => edit(grid, window, |layout| swap(layout, a, b))?,
        GridMsg::Drop {
            window,
            pane,
            at,
            size,
        } => {
            edit(grid, window, |layout| land(layout, side, pane, at, size))?;
            handled(grid, window, pane);
        }
        GridMsg::Moved { window, frame } => {
            let window = find(grid, window)?;
            window.frame = Some(frame).filter(Frame::fits).or(window.frame);
        }
    }
    Ok(Vec::new())
}

fn find(grid: &mut Windows, id: WindowId) -> Result<&mut Window, GridError> {
    let found = grid.windows.iter_mut().find(|window| window.id == id);
    found.ok_or(GridError::NoSuchWindow(id))
}

/// Changes one window's layout. One whose tracks or slots came out different is the saved
/// layout no longer; a pane going out or coming back to its slot leaves it the saved one.
fn edit(
    grid: &mut Windows,
    id: WindowId,
    change: impl FnOnce(&mut Layout) -> Result<(), GridError>,
) -> Result<(), GridError> {
    let window = find(grid, id)?;
    // A slot is named by its stack's first tab: which tab is in front is no change of layout.
    let shape = |layout: &Layout| {
        let named =
            |(pane, at): &(PaneId, _)| (layout.stack(*pane).first().copied().unwrap_or(*pane), *at);
        let areas: Vec<_> = layout.areas.iter().map(named).collect();
        (layout.cols.clone(), layout.rows.clone(), areas)
    };
    let before = shape(&window.layout);
    change(&mut window.layout)?;
    if shape(&window.layout) != before {
        (window.saved, window.own) = (None, None);
    }
    Ok(())
}

/// Brings `pane` of `window` in front of its stack and back into the grid if its slot rests.
/// A tab changed is no change of layout: the window still shows the layout it showed.
fn raise(grid: &mut Windows, window: WindowId, pane: PaneId) -> Result<(), GridError> {
    let layout = &mut find(grid, window)?.layout;
    front(layout, pane)?;
    if layout.is_resting(pane) {
        rejoin(layout, pane)?;
    }
    Ok(())
}

/// The pane holding the first slot of `layout` whose place is `place`: its stack's first
/// pane, or the pane alone, is of a kind that goes there. A pulled-out pane's slot is none.
fn host(grid: &Windows, layout: &Layout, place: Place) -> Option<PaneId> {
    let first = |held: PaneId| layout.stack(held).first().copied().unwrap_or(held);
    let placed = |held: &PaneId| {
        let here = !layout.pulled(*held) && grid.origin_of(*held).is_none();
        here && grid.kind(first(*held)).map(PaneKind::place) == Some(place)
    };
    layout.areas.iter().map(|(pane, _)| *pane).find(placed)
}

/// Shows a pane of `kind` in `id`, the person's from then on. One the window has comes in
/// front; else a hidden or a new one joins the stack of its place, or opens that place.
fn open(grid: &mut Windows, id: WindowId, kind: PaneKind) -> Result<Vec<WindowOp>, GridError> {
    let layout = &grid.window(id).ok_or(GridError::NoSuchWindow(id))?.layout;
    let of_kind = |pane: &PaneId| grid.kind(*pane) == Some(kind);
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
    let onto = host(grid, layout, kind.place());
    let size = grid.room(id);
    let placed = place(layout, size, grid.side);
    let largest = placed
        .into_iter()
        .max_by(|(_, a), (_, b)| (a.w * a.h).total_cmp(&(b.w * b.h)));
    let hidden = grid.panes.iter().find(|pane| pane.kind == kind);
    let hidden = hidden.map(|pane| pane.id).filter(|pane| idle(grid, *pane));
    let pane = match hidden {
        Some(pane) => pane,
        None => add(grid, kind)?,
    };
    fresh(grid, id, pane);
    if let Some(onto) = onto {
        stack(&mut find(grid, id)?.layout, onto, pane)?;
        raise(grid, id, pane)?;
        return Ok(Vec::new());
    }
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
    let at = match kind.place() {
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
    Ok(Vec::new())
}

/// The person asked for `pane` in `window` or moved it there: it is theirs, open in the
/// session the window shows, and no longer follows what its kind has.
fn handled(grid: &mut Windows, window: WindowId, pane: PaneId) {
    let thread = grid.shows.get(&window).cloned();
    let Some(pane) = grid.panes.iter_mut().find(|other| other.id == pane) else {
        return;
    };
    match (pane.seated, thread) {
        // The chat a layout seated is the window's own: it shows whatever session is shown.
        (true, _) if pane.kind == PaneKind::Chat => pane.scope = Scope::Window,
        (true, thread) => pane.scope = Scope::new(pane.kind, thread),
        (false, Some(thread)) => pane.scope.open(thread),
        (false, None) => {}
    }
    pane.seated = false;
}

/// A pane the person just made in `window`: theirs, open in the session it shows.
fn fresh(grid: &mut Windows, window: WindowId, pane: PaneId) {
    let thread = grid.shows.get(&window).cloned();
    if let Some(pane) = grid.panes.iter_mut().find(|other| other.id == pane) {
        (pane.seated, pane.scope) = (false, Scope::new(pane.kind, thread));
    }
}

/// Settles each slot of `id` on what shows here: its front pane if it shows, else the first
/// tab that does, else the slot rests. A pulled-out pane's slot stays; untold, nothing moves.
fn settle(grid: &mut Windows, id: WindowId) -> Result<(), GridError> {
    settle_in(grid, id, None)
}

/// [`settle`] for the one slot `only` is in, or for every slot of the window.
fn settle_in(grid: &mut Windows, id: WindowId, only: Option<PaneId>) -> Result<(), GridError> {
    if !grid.have.contains_key(&id) {
        return Ok(());
    }
    let layout = &grid.window(id).ok_or(GridError::NoSuchWindow(id))?.layout;
    let here = |held: &PaneId| !layout.pulled(*held);
    let guest = |pane: PaneId| grid.origin_of(pane).is_some();
    let slots = layout.areas.iter().map(|(pane, _)| *pane).filter(here);
    let slots = slots.filter(|held| !guest(*held));
    let slots = slots.filter(|held| only.is_none_or(|only| layout.front(only) == Some(*held)));
    let next = |held: PaneId| {
        let shows = |pane: &PaneId| !layout.pulled(*pane) && grid.showing(id, *pane);
        let front = Some(held).filter(shows);
        (
            held,
            front.or_else(|| layout.stack(held).iter().copied().find(shows)),
        )
    };
    let slots: Vec<(PaneId, Option<PaneId>)> = slots.map(next).collect();
    let layout = &mut find(grid, id)?.layout;
    for (held, next) in slots {
        match next {
            // A pane the grid cannot close up round stays in.
            None if !layout.is_away(held) => drop(rest(layout, held).ok()),
            None => {}
            Some(next) => {
                front(layout, next)?;
                if layout.is_resting(next) {
                    rejoin(layout, next)?;
                }
            }
        }
    }
    Ok(())
}

/// A new pane of `kind`, numbered one past the highest there is.
fn add(grid: &mut Windows, kind: PaneKind) -> Result<PaneId, GridError> {
    let highest = grid.panes.iter().map(|pane| pane.id.0).max().unwrap_or(0);
    let id = highest.checked_add(1).map(PaneId).ok_or(GridError::Full)?;
    let (seated, scope) = (false, Scope::Window);
    grid.panes.push(Pane {
        id,
        kind,
        seated,
        scope,
    });
    Ok(id)
}

/// Whether `pane` is in no window and out of none: hidden, for any window to show again.
fn idle(grid: &Windows, pane: PaneId) -> bool {
    !grid.windows.iter().any(|window| window.layout.holds(pane))
}

/// Takes `pane` out of the window it is drawn in, for good. A window left with no pane
/// goes too, unless panes are out of it and would have nowhere to come back to.
fn lift(grid: &mut Windows, pane: PaneId) -> Result<Vec<WindowOp>, GridError> {
    let from = grid.window_of(pane).ok_or(GridError::NoSuchPane(pane))?;
    let window = find(grid, from)?;
    if window.layout.docked().nth(1).is_some() {
        edit(grid, from, |layout| close(layout, pane))?;
        return Ok(Vec::new());
    }
    if !window.layout.away.is_empty() {
        return Err(GridError::LastPane(pane));
    }
    grid.windows.retain(|window| window.id != from);
    Ok(vec![WindowOp::Close(from)])
}

/// Keeps the window's arrangement as drawn under a name and the lowest number free.
fn keep(grid: &mut Windows, id: WindowId, name: &str) -> Result<(), GridError> {
    let name = named(name).ok_or(GridError::NoName)?;
    let numbers = FIRST_OWN..FIRST_OWN.saturating_add(MAX_OWN);
    let mut free = numbers
        .map(OwnId)
        .filter(|n| grid.own.iter().all(|kept| kept.id != *n));
    let number = free.next().ok_or(GridError::TooManyLayouts)?;
    let window = grid.window(id).ok_or(GridError::NoSuchWindow(id))?;
    let shape = Shape::of(&window.layout, |pane| grid.kind(pane));
    let shape = shape.ok_or(GridError::NotTiled)?;
    grid.own.push(Own {
        id: number,
        name,
        shape,
    });
    let window = find(grid, id)?;
    (window.saved, window.own) = (None, Some(number));
    Ok(())
}

/// Shows a saved layout in a window.
fn show(grid: &mut Windows, id: WindowId, slug: &str) -> Result<Vec<WindowOp>, GridError> {
    let saved = find_saved(slug).ok_or(GridError::NoSuchLayout)?;
    wear(grid, id, &saved.shape(), (Some(saved.slug), None))
}

/// Shows a layout in a window, marked as the one shown. Each slot goes to a pane of its kind
/// (drawn here, else out of here, else hidden, else new), which rests while its kind has nothing.
fn wear(
    grid: &mut Windows,
    id: WindowId,
    saved: &Shape,
    (slug, own): (Option<Slug>, Option<OwnId>),
) -> Result<Vec<WindowOp>, GridError> {
    let old = grid
        .window(id)
        .map(|window| window.layout.clone())
        .unwrap_or_default();
    let drawn = |pane: PaneId| old.docked().any(|(other, _)| other == pane);
    let here = |pane: PaneId| old.holds(pane);
    let mut seated: Vec<PaneId> = Vec::new();
    for (kind, _) in &saved.areas {
        let free = |pane: &&Pane| pane.kind == *kind && !seated.contains(&pane.id);
        let pick = |fits: &dyn Fn(PaneId) -> bool| {
            let found = grid.panes.iter().filter(free).find(|pane| fits(pane.id));
            found.map(|pane| pane.id)
        };
        let known = pick(&drawn)
            .or_else(|| pick(&here))
            .or_else(|| pick(&|pane| idle(grid, pane)));
        let pane = match known {
            Some(pane) => pane,
            None => add(grid, *kind)?,
        };
        seated.push(pane);
    }
    let seats: BTreeSet<PaneId> = seated.iter().copied().collect();
    let mut layout = saved.seat(seated);
    let guest = |pane: &PaneId| grid.origin_of(*pane).is_some() && layout.slot(*pane).is_none();
    if let Some(pane) = old.docked().map(|(pane, _)| pane).find(guest) {
        return Err(GridError::Stranded(pane));
    }
    let mut ops = Vec::new();
    for hole in old.away {
        // A layout that cannot close up round a pane that is out takes it back in.
        if hole.resting {
            // A resting pane the layout has no slot for is hidden; one it has rests on.
            rest(&mut layout, hole.pane).ok();
            continue;
        }
        match (layout.slot(hole.pane), leave(&mut layout, hole.pane)) {
            (None, _) => layout.away.push(hole),
            (Some(_), Ok(())) => {}
            (Some(_), Err(GridError::LastPane(pane))) => ops.extend(lift(grid, pane)?),
            (Some(_), Err(error)) => return Err(error),
        }
    }
    for pane in grid.panes.iter_mut() {
        pane.seated = pane.seated || seats.contains(&pane.id);
    }
    match find(grid, id) {
        Ok(window) => (window.layout, window.saved, window.own) = (layout, slug, own),
        Err(_) => {
            grid.windows.push(Window {
                id,
                layout,
                saved: slug,
                own,
                frame: None,
            });
            ops.push(WindowOp::Open(id));
        }
    }
    settle(grid, id)?;
    Ok(ops)
}

/// Pulls `pane` out into a window of one. A pane already out of another window is a guest
/// where it is drawn: it leaves no slot behind there.
fn pull(grid: &mut Windows, pane: PaneId) -> Result<Vec<WindowOp>, GridError> {
    let from = grid.window_of(pane).ok_or(GridError::NoSuchPane(pane))?;
    let highest = grid
        .windows
        .iter()
        .map(|window| window.id.0)
        .max()
        .unwrap_or(0);
    let id = highest
        .checked_add(1)
        .map(WindowId)
        .ok_or(GridError::Full)?;
    let guest = grid.origin_of(pane).is_some();
    let layout = &mut find(grid, from)?.layout;
    if let Some(next) = next_tab(layout, pane).filter(|next| !layout.is_away(*next)) {
        // A tab pulled out leaves its slot to the next and keeps its place in the stack.
        front(layout, next)?;
        let (beside, resting) = (Vec::new(), false);
        layout.away.push(Hole {
            pane,
            beside,
            resting,
        });
        settle_in(grid, from, Some(next))?;
    } else {
        let out = |layout: &mut Layout| {
            if guest {
                close(layout, pane)
            } else {
                leave(layout, pane)
            }
        };
        edit(grid, from, out)?;
    }
    grid.windows.push(Window {
        id,
        layout: Layout::single(pane),
        saved: None,
        own: None,
        frame: None,
    });
    Ok(vec![WindowOp::Open(id)])
}

/// Docks a pulled-out `pane` back into the window it left.
fn dock(grid: &mut Windows, pane: PaneId) -> Result<Vec<WindowOp>, GridError> {
    let home = grid.origin_of(pane).ok_or(GridError::NoSuchPane(pane))?;
    let ops = lift(grid, pane)?;
    edit(grid, home, |layout| rejoin(layout, pane))?;
    Ok(ops)
}

/// Closes `pane` in the session its window shows when it is open in others too: it rests
/// here and keeps its slot for them. True when that was all there was to close.
fn shut_here(grid: &mut Windows, pane: PaneId) -> Result<bool, GridError> {
    let Some(window) = grid
        .window_of(pane)
        .filter(|_| grid.origin_of(pane).is_none())
    else {
        return Ok(false);
    };
    let Some(thread) = grid.shows.get(&window).cloned() else {
        return Ok(false);
    };
    let found = grid.panes.iter_mut().find(|other| other.id == pane);
    let Some(Scope::Sessions(list)) = found.filter(|pane| !pane.seated).map(|p| &mut p.scope)
    else {
        return Ok(false);
    };
    if !list.iter().any(|other| *other != thread) {
        return Ok(false);
    }
    list.retain(|other| *other != thread);
    settle_in(grid, window, Some(pane))?;
    Ok(true)
}

/// Closes `pane` for good: out of its window, out of the window it left, and forgotten. A
/// pane open in other sessions too only closes in this one.
fn shut(grid: &mut Windows, pane: PaneId) -> Result<Vec<WindowOp>, GridError> {
    if shut_here(grid, pane)? {
        return Ok(Vec::new());
    }
    // A tab among others: it goes, and the next one that shows here takes the slot.
    let mate = |grid: &Windows, window| next_tab(&grid.window(window)?.layout, pane);
    if let Some(window) = grid.home_of(pane)
        && let Some(mate) = mate(grid, window)
        && unstack(&mut find(grid, window)?.layout, pane)
    {
        grid.panes.retain(|other| other.id != pane);
        settle_in(grid, window, Some(mate))?;
        // The window keeps a pane in: its last tab here does not close while others are out.
        find(grid, window)?.layout.check()?;
        return Ok(Vec::new());
    }
    let home = grid.origin_of(pane);
    let ops = lift(grid, pane)?;
    if let Some(home) = home {
        edit(grid, home, |layout| close(layout, pane))?;
    }
    grid.panes.retain(|other| other.id != pane);
    Ok(ops)
}

#[cfg(test)]
mod random;
#[cfg(test)]
mod resting;
#[cfg(test)]
mod sessions;
#[cfg(test)]
mod stacks;
#[cfg(test)]
mod tests;
