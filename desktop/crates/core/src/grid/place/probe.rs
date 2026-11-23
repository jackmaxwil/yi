//! What the grid's tests measure a placement with: an exact tiling, the pane minimum, and
//! one pane's rectangle.

use std::error::Error;

use crate::grid::layout::{Layout, MIN_PANE, Rect, Size};
use crate::grid::saved::Saved;
use crate::ids::PaneId;
use crate::pane::PaneKind;

pub(crate) type Check = Result<(), Box<dyn Error>>;

/// The pane grid of the record's frame 1: the 1200 by 750 window with the folded sidebar.
pub(crate) const FRAME_1: Size = Size {
    w: 1034.0,
    h: 676.0,
};

/// Two edges nearer than this are one edge: f32 sums of a few dozen track widths.
const NEAR: f32 = 0.01;

pub(crate) fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

/// Whether two rectangles are the same but for rounding, where a width was split by ratio.
pub(crate) fn near(a: Rect, b: Rect) -> bool {
    let pairs = [(a.x, b.x), (a.y, b.y), (a.w, b.w), (a.h, b.h)];
    pairs.iter().all(|(a, b)| (a - b).abs() < NEAR)
}

/// The pane a test gives a kind: its place in the kind list, counted from one.
pub(crate) fn pane(kind: PaneKind) -> PaneId {
    PaneId(kind as u32 + 1)
}

pub(crate) fn open(saved: &Saved) -> Layout {
    saved.open(pane)
}

pub(crate) fn find(placed: &[(PaneId, Rect)], pane: PaneId) -> Result<Rect, String> {
    let found = placed.iter().find(|(other, _)| *other == pane);
    found
        .map(|(_, rect)| *rect)
        .ok_or(format!("{pane:?} has no rectangle"))
}

/// A rectangle's start and length along one axis.
pub(crate) type Along = fn(&Rect) -> (f32, f32);

/// The distinct edges of the rectangles along one axis, in order.
pub(crate) fn edges(placed: &[(PaneId, Rect)], of: Along) -> Vec<f32> {
    let mut all: Vec<f32> = placed
        .iter()
        .flat_map(|(_, rect)| {
            let (start, length) = of(rect);
            [start, start + length]
        })
        .collect();
    all.sort_by(f32::total_cmp);
    all.dedup_by(|next, kept| (*next - *kept).abs() < NEAR);
    all
}

pub(crate) fn across(rect: &Rect) -> (f32, f32) {
    (rect.x, rect.w)
}

pub(crate) fn down(rect: &Rect) -> (f32, f32) {
    (rect.y, rect.h)
}

/// Fails unless the rectangles cover a grid of `size` exactly: no pane empty or outside, and
/// every cell between neighbouring edges inside one rectangle and only one.
pub(crate) fn tiled(placed: &[(PaneId, Rect)], size: Size) -> Result<(), String> {
    let (xs, ys) = (edges(placed, across), edges(placed, down));
    let spans = |edges: &[f32], end: f32| match (edges.first(), edges.last()) {
        (Some(first), Some(last)) => first.abs() < NEAR && (last - end).abs() < NEAR,
        _ => false,
    };
    if !spans(&xs, size.w) || !spans(&ys, size.h) {
        return Err(format!(
            "the panes span {xs:?} by {ys:?}, not {size:?}: {placed:?}"
        ));
    }
    if let Some((pane, rect)) = placed.iter().find(|(_, r)| r.w < NEAR || r.h < NEAR) {
        return Err(format!("{pane:?} is empty: {rect:?}"));
    }
    for (left, right) in xs.iter().zip(xs.iter().skip(1)) {
        for (top, bottom) in ys.iter().zip(ys.iter().skip(1)) {
            let (x, y) = ((left + right) / 2.0, (top + bottom) / 2.0);
            let over = |rect: &Rect| {
                rect.x < x && x < rect.x + rect.w && rect.y < y && y < rect.y + rect.h
            };
            let panes: Vec<PaneId> = placed
                .iter()
                .filter(|(_, rect)| over(rect))
                .map(|(pane, _)| *pane)
                .collect();
            if panes.len() != 1 {
                return Err(format!("at {x}, {y} there are {panes:?}: {placed:?}"));
            }
        }
    }
    Ok(())
}

/// Fails if a pane is under the minimum along an axis that has room for the minimum of
/// every track drawn on it.
pub(crate) fn roomy(placed: &[(PaneId, Rect)], size: Size) -> Result<(), String> {
    let axes: [(Along, f32, f32); 2] = [(across, size.w, MIN_PANE.w), (down, size.h, MIN_PANE.h)];
    for (of, length, min) in axes {
        let tracks = edges(placed, of).len().saturating_sub(1);
        let needed = min * f32::from(u16::try_from(tracks).unwrap_or(u16::MAX));
        if needed > length + NEAR {
            continue;
        }
        if let Some((pane, rect)) = placed.iter().find(|(_, r)| of(r).1 < min - NEAR) {
            return Err(format!(
                "{pane:?} is under the {min} minimum with room for {tracks} tracks in {length}: {rect:?}"
            ));
        }
    }
    Ok(())
}
