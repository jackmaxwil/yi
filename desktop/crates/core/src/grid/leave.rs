//! A pane leaves the grid and comes back. No hole, ever: the tracks it alone held collapse,
//! or the panes along one of its edges take its room.

use crate::grid::layout::{
    Area, Axis, Edge, GridError, Hole, Layout, MAX_PANES, MAX_TRACKS, Track, tiles,
};
use crate::ids::PaneId;

/// A layout as it is drawn: the panes that are in, grown over the slots of those that are out.
pub(crate) struct Shown {
    /// The layout's own index of each column still in use, and of each row.
    pub cols: Vec<usize>,
    pub rows: Vec<usize>,
    /// Each pane's area, between the lines of the tracks still in use.
    pub areas: Vec<(PaneId, Area)>,
}

impl Shown {
    /// The layout with every slot drawn, whoever is out.
    pub(crate) fn whole(layout: &Layout) -> Shown {
        Shown {
            cols: (0..layout.cols.len()).collect(),
            rows: (0..layout.rows.len()).collect(),
            areas: layout.areas.clone(),
        }
    }

    pub(crate) fn kept(&self, axis: Axis) -> &[usize] {
        match axis {
            Axis::Cols => &self.cols,
            Axis::Rows => &self.rows,
        }
    }

    /// The tracks still in use along `axis`, in order.
    pub(crate) fn tracks(&self, layout: &Layout, axis: Axis) -> Vec<Track> {
        let list = match axis {
            Axis::Cols => &layout.cols,
            Axis::Rows => &layout.rows,
        };
        let kept = self.kept(axis).iter();
        kept.filter_map(|index| list.get(*index).copied()).collect()
    }
}

/// The layout as it is drawn, each pane that is out taken from it in the order they left.
pub(crate) fn close_up(layout: &Layout) -> Result<Shown, GridError> {
    let mut shown = Shown::whole(layout);
    for hole in &layout.away {
        if layout.slot(hole.pane).is_some() {
            take(
                &mut shown.cols,
                &mut shown.rows,
                &mut shown.areas,
                hole.pane,
            )?;
        }
    }
    Ok(shown)
}

/// Whether `area` lies against `edge` of `hole`, sharing some of that edge.
fn touches(area: Area, hole: Area, edge: Edge) -> bool {
    let (axis, first) = edge.parts();
    let ((start, end), (from, to)) = (area.span(axis), hole.span(axis));
    let ((low, high), (near, far)) = (area.span(axis.other()), hole.span(axis.other()));
    (if first { end == from } else { start == to }) && low < far && high > near
}

/// Whether `area` stays between the lines `hole` spans along `axis`.
fn within(area: Area, hole: Area, axis: Axis) -> bool {
    let ((start, end), (from, to)) = (area.span(axis), hole.span(axis));
    from <= start && end <= to
}

/// The panes with `pane` gone and those along `edge` of its slot grown over it, if any lie
/// there. They tile only where those panes cover that edge and no more: the caller checks.
fn grow(
    areas: &[(PaneId, Area)],
    pane: PaneId,
    hole: Area,
    edge: Edge,
) -> Option<Vec<(PaneId, Area)>> {
    let (axis, first) = edge.parts();
    let (from, to) = hole.span(axis);
    let over = |area: Area| {
        let (start, end) = area.span(axis);
        area.with(axis, if first { (start, to) } else { (from, end) })
    };
    let rest = areas.iter().copied().filter(|(other, _)| *other != pane);
    let near = |area: Area| touches(area, hole, edge);
    let any = rest.clone().any(|(_, area)| near(area));
    let grown = rest.map(|(other, area)| (other, if near(area) { over(area) } else { area }));
    any.then(|| grown.collect())
}

/// Takes `pane` out of a tiling and closes the gap. Tracks it alone held go; otherwise the
/// panes along one edge grow over it, the first edge that leaves a tiling the next can cut.
fn take<T>(
    cols: &mut Vec<T>,
    rows: &mut Vec<T>,
    areas: &mut Vec<(PaneId, Area)>,
    pane: PaneId,
) -> Result<(), GridError> {
    let found = areas.iter().find(|(other, _)| *other == pane);
    let gone = found
        .map(|(_, area)| *area)
        .ok_or(GridError::NoSuchPane(pane))?;
    if areas.len() < 2 {
        return Err(GridError::LastPane(pane));
    }
    let all = |len: usize| (1, u16::try_from(len).unwrap_or(u16::MAX).saturating_add(1));
    let (all_cols, all_rows) = (all(cols.len()), all(rows.len()));
    let axes = [
        (Axis::Cols, &mut *cols, all_cols, all_rows),
        (Axis::Rows, &mut *rows, all_rows, all_cols),
    ];
    for (axis, list, whole, across) in axes {
        let (start, end) = gone.span(axis);
        if gone.span(axis.other()) != across || (start, end) == whole {
            continue;
        }
        let (low, high) = (
            usize::from(start.saturating_sub(1)),
            usize::from(end.saturating_sub(1)),
        );
        if low >= high || high > list.len() {
            return Err(GridError::NotTiled);
        }
        list.drain(low..high);
        let cut = end.saturating_sub(start);
        let back = |line: u16| {
            if line >= end {
                line.saturating_sub(cut)
            } else {
                line.min(start)
            }
        };
        areas.retain(|(other, _)| *other != pane);
        for (_, area) in areas.iter_mut() {
            let (from, to) = area.span(axis);
            *area = area.with(axis, (back(from), back(to)));
        }
        return Ok(());
    }
    let (n_rows, n_cols) = (rows.len(), cols.len());
    let fill =
        |edge: Edge| grow(areas, pane, gone, edge).filter(|grown| tiles(grown, n_rows, n_cols));
    *areas = Edge::ALL
        .into_iter()
        .find_map(fill)
        .ok_or(GridError::NotTiled)?;
    Ok(())
}

/// The neighbours of `pane` as drawn, each with the edge of it the pane sits on: first those
/// that cover a whole edge of the pane, which is where it would be put back.
fn beside(shown: &Shown, pane: PaneId) -> Vec<(PaneId, Edge)> {
    let found = shown.areas.iter().find(|(other, _)| *other == pane);
    let Some(hole) = found.map(|(_, area)| *area) else {
        return Vec::new();
    };
    let (mut exact, mut loose) = (Vec::new(), Vec::new());
    for edge in Edge::ALL {
        let along = || {
            shown
                .areas
                .iter()
                .filter(|(_, area)| touches(*area, hole, edge))
        };
        let fits = along().all(|(_, area)| within(*area, hole, edge.parts().0.other()));
        let list = if fits { &mut exact } else { &mut loose };
        list.extend(along().map(|(other, _)| (*other, edge.opposite())));
    }
    exact.extend(loose);
    exact
}

/// Takes `pane` out of the grid. Its slot stays for it and is closed up when drawn.
pub fn leave(layout: &mut Layout, pane: PaneId) -> Result<(), GridError> {
    let shown = close_up(layout)?;
    if !shown.areas.iter().any(|(other, _)| *other == pane) {
        return Err(GridError::NoSuchPane(pane));
    }
    layout.away.push(Hole {
        pane,
        beside: beside(&shown, pane),
        resting: false,
    });
    // Invariant: no hole, ever. A pane is out only if the grid closes up without it.
    let closed = close_up(layout).map(drop);
    if closed.is_err() {
        layout.away.pop();
    }
    closed
}

/// Takes `pane` out of the grid for want of anything to show, as [`leave`] does.
pub fn rest(layout: &mut Layout, pane: PaneId) -> Result<(), GridError> {
    leave(layout, pane)?;
    if let Some(hole) = layout.away.last_mut() {
        hole.resting = true;
    }
    Ok(())
}

/// Puts a pane that is out back: into its slot, or with no slot here, into half the room
/// of the first neighbour it remembers that is in, else of the first pane.
pub fn rejoin(layout: &mut Layout, pane: PaneId) -> Result<(), GridError> {
    let hole = layout.away.iter().find(|hole| hole.pane == pane);
    let hole = hole.cloned().ok_or(GridError::NoSuchPane(pane))?;
    if layout.slot(pane).is_none() {
        let is_in = |other: PaneId| layout.docked().any(|(docked, _)| docked == other);
        let known = hole.beside.iter().copied().find(|(other, _)| is_in(*other));
        let first = layout.docked().next().map(|(other, _)| (other, Edge::Left));
        let (other, edge) = known.or(first).ok_or(GridError::NoSuchPane(pane))?;
        split(layout, other, edge, pane)?;
    }
    layout.away.retain(|hole| hole.pane != pane);
    Ok(())
}

/// Makes a slot for `pane` on `edge` of `beside`, out of half of that pane's own: cut at a
/// line the slot already crosses, or through its one track, split in two.
pub fn split(
    layout: &mut Layout,
    beside: PaneId,
    edge: Edge,
    pane: PaneId,
) -> Result<(), GridError> {
    if layout.slot(pane).is_some() {
        return Err(GridError::AlreadyIn(pane));
    }
    if layout.areas.len() >= MAX_PANES {
        return Err(GridError::Full);
    }
    let at = layout.areas.iter().position(|(other, _)| *other == beside);
    let (at, whole) = at
        .zip(layout.slot(beside))
        .ok_or(GridError::NoSuchPane(beside))?;
    let (axis, first) = edge.parts();
    let (start, mut end) = whole.span(axis);
    if end.saturating_sub(start) < 2 {
        let list = match axis {
            Axis::Cols => &mut layout.cols,
            Axis::Rows => &mut layout.rows,
        };
        let index = usize::from(start.saturating_sub(1));
        let half = list.get(index).map(|track| track.sized(track.size() / 2.0));
        let half = half.ok_or(GridError::NotTiled)?;
        if list.len() >= MAX_TRACKS {
            return Err(GridError::Full);
        }
        list.insert(index, half);
        if let Some(track) = list.get_mut(index.saturating_add(1)) {
            *track = half;
        }
        let along = |line: u16| {
            if line > start {
                line.saturating_add(1)
            } else {
                line
            }
        };
        for (_, area) in layout.areas.iter_mut() {
            let (from, to) = area.span(axis);
            *area = area.with(axis, (along(from), along(to)));
        }
        end = end.saturating_add(1);
    }
    let half = end.saturating_sub(start) / 2;
    let cut = if first {
        start.saturating_add(half)
    } else {
        end.saturating_sub(half)
    };
    let (kept, given) = if first {
        ((cut, end), (start, cut))
    } else {
        ((start, cut), (cut, end))
    };
    if let Some((_, slot)) = layout.areas.get_mut(at) {
        *slot = whole.with(axis, kept);
    }
    layout
        .areas
        .insert(at.saturating_add(1), (pane, whole.with(axis, given)));
    Ok(())
}

/// Takes `pane` and its slot out for good; the rest fill the room.
pub fn close(layout: &mut Layout, pane: PaneId) -> Result<(), GridError> {
    if layout.slot(pane).is_some() {
        if !layout.docked().any(|(other, _)| other != pane) {
            return Err(GridError::LastPane(pane));
        }
        take(&mut layout.cols, &mut layout.rows, &mut layout.areas, pane)?;
    } else if !layout.is_away(pane) {
        return Err(GridError::NoSuchPane(pane));
    }
    layout.away.retain(|hole| hole.pane != pane);
    Ok(())
}

#[cfg(test)]
mod tests;
