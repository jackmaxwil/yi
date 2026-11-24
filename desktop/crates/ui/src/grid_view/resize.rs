//! Dragging the line between panes.

use rune::{Cursor, Drag, El, Tx, el};
use yd_core::grid::layout::{Axis, Point, Rect};
use yd_core::grid::place::REACH;
use yd_core::grid::windows::Windows;
use yd_core::ids::{PaneId, WindowId};

use crate::grid_view::drag::{At, Grab, hand};
use crate::win::Win;

/// Handles paint over the panes, so a press within reach of a line takes the line.
pub const OVER_PANES: i16 = 2;

/// One handle along an edge of `rect`: the right edge for columns, the bottom for rows.
fn handle(
    grid: &Windows,
    window: WindowId,
    at: At,
    pane: PaneId,
    rect: Rect,
    axis: Axis,
) -> Option<El> {
    let (edge, limit, middle, part, cursor) = match axis {
        Axis::Cols => (
            rect.x + rect.w,
            at.size.w,
            Point {
                x: rect.x + rect.w,
                y: rect.y + rect.h / 2.0,
            },
            "right",
            Cursor::ResizeColumn,
        ),
        Axis::Rows => (
            rect.y + rect.h,
            at.size.h,
            Point {
                x: rect.x + rect.w / 2.0,
                y: rect.y + rect.h,
            },
            "below",
            Cursor::ResizeRow,
        ),
    };
    // The grid's own edges are not lines between panes.
    if edge >= limit - REACH {
        return None;
    }
    let line = grid.line_at(window, at.size, middle)?;
    if line.axis != axis {
        return None;
    }
    let strip = match axis {
        Axis::Cols => el()
            .absolute(edge - REACH, rect.y)
            .size(REACH * 2.0, rect.h),
        Axis::Rows => el()
            .absolute(rect.x, edge - REACH)
            .size(rect.w, REACH * 2.0),
    };
    Some(
        strip
            .key(format!("grid.line.{part}-{}", pane.0))
            .paint_order(OVER_PANES)
            .cursor(cursor)
            .on_drag(move |win: &mut Win, drag: &mut Drag, tx: &mut Tx| {
                hand(win, Grab::Line(line), at, drag, tx);
            }),
    )
}

/// A drag handle on every line between two panes: rows first, so where a column line
/// crosses a row line the column takes the press (record 2.7.8).
pub fn handles(grid: &Windows, window: WindowId, at: At, placed: &[(PaneId, Rect)]) -> Vec<El> {
    [Axis::Rows, Axis::Cols]
        .into_iter()
        .flat_map(|axis| {
            placed
                .iter()
                .filter_map(move |(pane, rect)| handle(grid, window, at, *pane, *rect, axis))
        })
        .collect()
}
