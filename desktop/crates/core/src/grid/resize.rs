//! Dragging the line between two panes; double-click evens the two sides.

use crate::grid::layout::{Axis, GridError, Layout, Line, MIN_PANE, Size, Track};
use crate::grid::leave::{Shown, close_up};
use crate::grid::place::{Measure, extents, joined};

/// A line as drawn: the tracks in use measured, and the run of tracks either side of the
/// line, which no pane edge divides and so moves as one.
struct Reach {
    shown: Shown,
    measure: Measure,
    first: usize,
    at: usize,
    last: usize,
    before: f32,
    after: f32,
    min: f32,
}

fn reach(layout: &Layout, line: Line, size: Size) -> Result<Reach, GridError> {
    let shown = close_up(layout)?;
    let (length, min) = match line.axis {
        Axis::Cols => (size.w, MIN_PANE.w),
        Axis::Rows => (size.h, MIN_PANE.h),
    };
    let joined = joined(&shown, line.axis);
    let measure = extents(length, &shown.tracks(layout, line.axis), &joined, min);
    let mut kept = shown.kept(line.axis).iter();
    let at = kept.rposition(|index| *index <= usize::from(line.after));
    let at = at
        .filter(|at| joined.get(*at) == Some(&false))
        .ok_or(GridError::NoSuchLine)?;
    let held = |index: &usize| joined.get(*index) == Some(&true);
    let first = (0..at).rev().take_while(held).last().unwrap_or(at);
    let next = at.saturating_add(1);
    let last = (next..joined.len())
        .take_while(held)
        .last()
        .map_or(next, |index| index.saturating_add(1));
    let tracks = &measure.tracks;
    let width = |from: usize, to: usize| Some(tracks.get(to)?.1 - tracks.get(from)?.0);
    let (before, after) = width(first, at)
        .zip(width(next, last))
        .ok_or(GridError::NoSuchLine)?;
    Ok(Reach {
        shown,
        measure,
        first,
        at,
        last,
        before,
        after,
        min,
    })
}

/// Moves `line` by `by` points in a grid of `size`; only that line moves, and it stops where
/// a pane beside it would go under the minimum.
pub fn resize(layout: &mut Layout, line: Line, by: f32, size: Size) -> Result<(), GridError> {
    let reach = reach(layout, line, size)?;
    let spare = |width: f32| (width - reach.min).max(0.0);
    let by = if by.is_finite() {
        by.max(-spare(reach.before)).min(spare(reach.after))
    } else {
        0.0
    };
    if by == 0.0 || reach.before <= 0.0 || reach.after <= 0.0 {
        return Ok(());
    }
    let kept = reach.shown.kept(line.axis);
    let list = match line.axis {
        Axis::Cols => &mut layout.cols,
        Axis::Rows => &mut layout.rows,
    };
    let drawn = |place: usize, (start, end): &(f32, f32)| {
        let width = end - start;
        if (reach.first..=reach.at).contains(&place) {
            width + by * (width / reach.before)
        } else if (reach.at.saturating_add(1)..=reach.last).contains(&place) {
            width - by * (width / reach.after)
        } else {
            width
        }
    };
    let extents = reach.measure.tracks.iter().enumerate();
    let widths = extents.map(|(place, extent)| drawn(place, extent));
    let widths: Vec<(usize, f32)> = kept.iter().copied().zip(widths).collect();
    // The tracks beside the line take their new widths in their own units, at the scale the
    // axis is drawn at. No other track is written, so each keeps the size it was given.
    let count = reach.last.saturating_sub(reach.first).saturating_add(1);
    let sized = |(index, width): &(usize, f32)| {
        let track = reach.measure.sized(*list.get(*index)?, *width)?;
        Some((*index, track))
    };
    let beside = widths.iter().skip(reach.first).take(count).map(sized);
    let Some(beside) = beside.collect::<Option<Vec<(usize, Track)>>>() else {
        keep_as_drawn(list, kept, &reach.measure, &widths);
        return Ok(());
    };
    for (index, track) in beside {
        if let Some(slot) = list.get_mut(index) {
            *slot = track;
        }
    }
    Ok(())
}

/// Writes every track in use at the width it is drawn. A share beside a dragged line can
/// hold no room, in a grid narrower than its fixed tracks, and then cannot say its width.
fn keep_as_drawn(list: &mut [Track], kept: &[usize], measure: &Measure, widths: &[(usize, f32)]) {
    let shared = |(place, index): (usize, &usize)| match list.get(*index) {
        Some(Track::Fr(fr)) => {
            let (start, end) = measure.tracks.get(place)?;
            Some((end - start, *fr))
        }
        Some(Track::Px(_)) | None => None,
    };
    let both = |sum: (f32, f32), each: (f32, f32)| (sum.0 + each.0, sum.1 + each.1);
    let (points, shares) = kept
        .iter()
        .enumerate()
        .filter_map(shared)
        .fold((0.0, 0.0), both);
    let worth = if points > 0.0 && shares > 0.0 {
        points / shares
    } else {
        1.0
    };
    for (index, track) in list.iter_mut().enumerate() {
        let drawn = widths.iter().find(|(kept, _)| *kept == index);
        *track = match (drawn, *track) {
            (Some((_, width)), track) => track.sized(*width),
            // Shares out of view follow into points, held to the largest number there is.
            (None, Track::Fr(fr)) if fr > 0.0 => Track::Fr((fr * worth).min(f32::MAX)),
            (None, track) => track,
        };
    }
}

/// Gives the panes either side of `line` equal room.
pub fn even(layout: &mut Layout, line: Line, size: Size) -> Result<(), GridError> {
    let reach = reach(layout, line, size)?;
    resize(layout, line, (reach.after - reach.before) / 2.0, size)
}

/// Where `line` is drawn, in points from the grid's left or top, before any mirroring.
pub(crate) fn position(layout: &Layout, line: Line, size: Size) -> Result<f32, GridError> {
    let reach = reach(layout, line, size)?;
    let end = reach.measure.tracks.get(reach.at).map(|(_, end)| *end);
    end.ok_or(GridError::NoSuchLine)
}

#[cfg(test)]
mod tests;
