//! Tracks to points and slots to rectangles (record 2.7.2, 2.7.3, 2.7.6), closed up round
//! the panes that are out.

use crate::grid::layout::{Area, Axis, ChatSide, Layout, Line, MIN_PANE, Point, Rect, Size, Track};
use crate::grid::leave::{Shown, close_up};
use crate::ids::PaneId;

/// How near a pane's edge, in points, the pointer takes hold of the line there (record 2.7.8).
pub const REACH: f32 = 3.0;

/// The start and end of each track along an axis `size` points long. The tracks always fill
/// the axis: fixed ones shrink together once shares would fall under `min` each.
pub fn tracks(size: f32, list: &[Track], min: f32) -> Vec<(f32, f32)> {
    extents(size, list, &[], min).tracks
}

/// An axis measured: where each track in use starts and ends, and how wide a fixed point
/// and a share are drawn there, so a drag can be written back in a track's own units.
pub(crate) struct Measure {
    pub tracks: Vec<(f32, f32)>,
    /// Fixed tracks draw `points / fixed.1 * fixed.0` wide: as written, unless scaled to fit.
    fixed: (f32, f32),
    /// Shares draw `share / shared.1 * shared.0` wide; a unit of nothing where they hold no room.
    shared: (f32, f32),
}

impl Measure {
    /// A track of the same kind as `track` that draws `width` wide here. None for a share
    /// where shares hold no room: no share can say a width then.
    pub(crate) fn sized(&self, track: Track, width: f32) -> Option<Track> {
        let ((scale, most), (unit, top)) = (self.fixed, self.shared);
        match track {
            Track::Px(_) => Some(Track::Px(width / scale * most)),
            Track::Fr(_) => (unit > 0.0).then(|| Track::Fr(width / unit * top)),
        }
    }
}

/// Tracks with no pane edge between them, sized as one so a pane across them has one
/// minimum: their fixed points, and their shares as parts of the largest share on the axis.
#[derive(Clone, Copy, Default)]
struct Run {
    px: f32,
    fr: f32,
    len: usize,
}

/// No grid is this many points long: sizes past it are held to it, so no sum overflows.
const VAST: f32 = 1.0e9;

fn count(len: usize) -> f32 {
    u16::try_from(len).map_or(f32::MAX, f32::from)
}

/// `tracks`, where `joined[i]` says no pane ends on the line after track `i`.
pub(crate) fn extents(size: f32, list: &[Track], joined: &[bool], min: f32) -> Measure {
    let clean = |value: f32| {
        if value.is_finite() {
            value.clamp(0.0, VAST)
        } else {
            0.0
        }
    };
    let size = clean(size);
    // Invariant: every size comes out finite. A share counts as a part of the largest one,
    // so shares as small or as large as numbers go still divide the axis between them.
    let top = list.iter().fold(0.0, |top: f32, track| match track {
        Track::Fr(fr) if fr.is_finite() => top.max(*fr),
        Track::Fr(_) | Track::Px(_) => top,
    });
    let parts = |track: &Track| match track {
        Track::Px(px) => (clean(*px), 0.0),
        Track::Fr(fr) if top > 0.0 && fr.is_finite() => (0.0, (fr / top).max(0.0)),
        Track::Fr(_) => (0.0, 0.0),
    };
    let mut runs: Vec<Run> = Vec::new();
    for (index, track) in list.iter().enumerate() {
        let before = index.checked_sub(1).and_then(|before| joined.get(before));
        if before != Some(&true) {
            runs.push(Run::default());
        }
        if let Some(run) = runs.last_mut() {
            let (px, fr) = parts(track);
            *run = Run {
                px: run.px + px,
                fr: run.fr + fr,
                len: run.len.saturating_add(1),
            };
        }
    }
    let (widths, unit, fixed) = share(size, &runs, min);
    let mut out = Vec::with_capacity(list.len());
    let mut rest = list.iter();
    let mut at = 0.0;
    for (run, width) in runs.iter().zip(widths) {
        let weigh = |track: &Track| parts(track).0 + parts(track).1 * unit;
        let weights: Vec<f32> = rest.by_ref().take(run.len).map(weigh).collect();
        let sum: f32 = weights.iter().sum();
        let (end, mut edge) = (at + width, at);
        for (index, weight) in weights.iter().enumerate() {
            let part = if sum > 0.0 {
                weight / sum
            } else {
                1.0 / count(run.len)
            };
            let next = if index.saturating_add(1) == run.len {
                end
            } else {
                edge + width * part
            };
            out.push((edge, next));
            edge = next;
        }
        at = end;
    }
    if let Some(last) = out.last_mut() {
        last.1 = size;
    }
    Measure {
        tracks: out,
        fixed,
        shared: (unit, top),
    }
}

/// The width of each run, the points a whole share is worth, and the scale on the fixed
/// ones: as asked while every run keeps `min`, then the fixed ones scaled, then all equal.
fn share(size: f32, runs: &[Run], min: f32) -> (Vec<f32>, f32, (f32, f32)) {
    let as_written = (1.0, 1.0);
    if count(runs.len()) * min >= size {
        let each = size / count(runs.len()).max(1.0);
        return (vec![each; runs.len()], 0.0, as_written);
    }
    let fixed: f32 = runs.iter().map(|run| run.px.max(min)).sum();
    if fixed <= size && runs.iter().any(|run| run.fr > 0.0) {
        let terms: Vec<(f32, f32)> = runs.iter().map(|run| (run.px, run.fr)).collect();
        let unit = level(size, min, &terms);
        let width = |run: &Run| (run.px + run.fr * unit).max(min);
        return (runs.iter().map(width).collect(), unit, as_written);
    }
    let top = runs.iter().fold(0.0, |top: f32, run| top.max(run.px));
    let part = |run: &Run| if top > 0.0 { run.px / top } else { 0.0 };
    let terms: Vec<(f32, f32)> = runs.iter().map(|run| (0.0, part(run))).collect();
    let scale = level(size, min, &terms);
    let width = |run: &Run| (part(run) * scale).max(min);
    let scaled = if scale > 0.0 {
        (scale, top)
    } else {
        as_written
    };
    (runs.iter().map(width).collect(), 0.0, scaled)
}

/// The `t` at which the terms `max(min, a + b * t)` add up to `size`: terms that would fall
/// under `min` are held there, one round at a time, and the rest share what is left.
fn level(size: f32, min: f32, terms: &[(f32, f32)]) -> f32 {
    let mut held = vec![false; terms.len()];
    let mut t = 0.0;
    for _ in 0..=terms.len() {
        let (mut fixed, mut slope) = (0.0, 0.0);
        for ((a, b), pinned) in terms.iter().zip(&held) {
            fixed += if *pinned { min } else { *a };
            slope += if *pinned { 0.0 } else { *b };
        }
        if slope <= 0.0 {
            return 0.0;
        }
        t = (size - fixed) / slope;
        let mut more = false;
        for ((a, b), pinned) in terms.iter().zip(held.iter_mut()) {
            if !*pinned && a + b * t < min {
                (*pinned, more) = (true, true);
            }
        }
        if !more {
            break;
        }
    }
    t
}

fn rect(area: Area, cols: &[(f32, f32)], rows: &[(f32, f32)]) -> Option<Rect> {
    let at = |list: &[(f32, f32)], line: u16, back: u16| {
        list.get(usize::from(line.checked_sub(back)?)).copied()
    };
    let (x, y) = (at(cols, area.c1, 1)?.0, at(rows, area.r1, 1)?.0);
    let (right, bottom) = (at(cols, area.c2, 2)?.1, at(rows, area.r2, 2)?.1);
    Some(Rect {
        x,
        y,
        w: right - x,
        h: bottom - y,
    })
}

/// Which lines between the tracks in use no pane ends on, by the track before each. In a
/// tiling a pane ends on a line just where another starts on it, so starts are enough.
pub(crate) fn joined(shown: &Shown, axis: Axis) -> Vec<bool> {
    let starts = |area: &Area| usize::from(area.span(axis).0);
    let ends = |line: usize| shown.areas.iter().any(|(_, area)| starts(area) == line);
    let lines = 2..=shown.kept(axis).len();
    lines.map(|line| !ends(line)).collect()
}

/// A layout closed up and measured: the panes in view, and each track in use, start to end.
struct Drawn {
    shown: Shown,
    cols: Vec<(f32, f32)>,
    rows: Vec<(f32, f32)>,
}

fn draw(layout: &Layout, size: Size) -> Drawn {
    // Invariant: a checked layout always closes up. One built wrong draws every slot instead.
    let shown = close_up(layout).unwrap_or_else(|_| Shown::whole(layout));
    let measure = |axis: Axis, length: f32, min: f32| {
        let tracks = shown.tracks(layout, axis);
        extents(length, &tracks, &joined(&shown, axis), min).tracks
    };
    let (cols, rows) = (
        measure(Axis::Cols, size.w, MIN_PANE.w),
        measure(Axis::Rows, size.h, MIN_PANE.h),
    );
    Drawn { shown, cols, rows }
}

/// Every pane's rectangle in a grid of `size`, in layout order: the panes that are in, with
/// the room of those that are out shared among them. Chat on the left mirrors every column.
pub fn place(layout: &Layout, size: Size, side: ChatSide) -> Vec<(PaneId, Rect)> {
    let drawn = draw(layout, size);
    let mirror = |rect: Rect| match side {
        ChatSide::Right => rect,
        ChatSide::Left => Rect {
            x: size.w - rect.x - rect.w,
            ..rect
        },
    };
    let slots = drawn.shown.areas.iter();
    slots
        .filter_map(|(pane, area)| Some((*pane, mirror(rect(*area, &drawn.cols, &drawn.rows)?))))
        .collect()
}

/// The line a pointer at `at` would drag: one within reach of an edge of the pane under it,
/// columns before rows. The grid's own edges and a line through that pane are not taken.
#[must_use]
pub fn line_at(layout: &Layout, size: Size, side: ChatSide, at: Point) -> Option<Line> {
    let drawn = draw(layout, size);
    let (x, y) = match side {
        ChatSide::Right => (at.x, at.y),
        ChatSide::Left => (size.w - at.x, at.y),
    };
    let under = |area: &Area| {
        let found = rect(*area, &drawn.cols, &drawn.rows);
        found.is_some_and(|r| r.x <= x && x <= r.x + r.w && r.y <= y && y <= r.y + r.h)
    };
    let (_, area) = drawn.shown.areas.iter().find(|(_, area)| under(area))?;
    let axes = [(Axis::Cols, x, &drawn.cols), (Axis::Rows, y, &drawn.rows)];
    axes.into_iter().find_map(|(axis, point, extents)| {
        let kept = drawn.shown.kept(axis);
        let (start, end) = area.span(axis);
        [start, end].into_iter().find_map(|line| {
            let before = usize::from(line).checked_sub(2)?;
            let near = (point - extents.get(before)?.1).abs() <= REACH;
            let after = u16::try_from(*kept.get(before)?).ok()?;
            (near && before.saturating_add(1) < kept.len()).then_some(Line { axis, after })
        })
    })
}

#[cfg(test)]
pub(crate) mod probe;
