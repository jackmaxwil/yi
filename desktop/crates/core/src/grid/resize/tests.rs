//! A line dragged, evened and found under the pointer, measured against record 2.7.8 and
//! the plan's minimums.

use crate::grid::layout::{
    Axis, ChatSide, Edge, GridError, Layout, Line, Point, Rect, Size, Track, area,
};
use crate::grid::leave::{leave, rejoin, split};
use crate::grid::place::probe::{Check, FRAME_1, find, near, open, pane, rect, roomy, tiled};
use crate::grid::place::{line_at, place};
use crate::grid::saved::{EVERYTHING, REVIEW};
use crate::ids::PaneId;
use crate::pane::PaneKind::{Changes, Chat, Code, Files, Remote, Report, Terminal, Web};

use super::{even, resize};

fn drawn(layout: &Layout, size: Size) -> Result<Vec<(PaneId, Rect)>, String> {
    let placed = place(layout, size, ChatSide::Right);
    tiled(&placed, size)?;
    Ok(placed)
}

fn after(axis: Axis, after: u16) -> Line {
    Line { axis, after }
}

/// Record 2.7.8: the line between web and the remote desktop is the line between the open
/// file and chat, so dragging it moves both; Changes, which spans it, does not change.
#[test]
fn dragging_a_line_moves_it_through_the_whole_grid() -> Check {
    let mut layout = open(&EVERYTHING);
    resize(&mut layout, after(Axis::Rows, 0), 30.0, FRAME_1)?;
    let placed = drawn(&layout, FRAME_1)?;
    let want = [
        (Web, rect(478.0, 0.0, 286.0, 256.0)),
        (Remote, rect(478.0, 256.0, 286.0, 160.0)),
        (Report, rect(478.0, 416.0, 286.0, 260.0)),
        (Code, rect(764.0, 0.0, 270.0, 256.0)),
        (Chat, rect(764.0, 256.0, 270.0, 420.0)),
        (Changes, rect(208.0, 0.0, 270.0, 416.0)),
        (Terminal, rect(208.0, 416.0, 270.0, 260.0)),
        (Files, rect(0.0, 0.0, 208.0, 676.0)),
    ];
    for (kind, rect) in want {
        assert_eq!(find(&placed, pane(kind))?, rect, "{kind:?}");
    }
    Ok(())
}

/// Plan default 6: a drag stops when the pane it squeezes is 160 wide or 120 high, however
/// far the pointer goes.
#[test]
fn a_drag_stops_at_the_smallest_pane() -> Check {
    let mut layout = open(&EVERYTHING);
    resize(&mut layout, after(Axis::Rows, 0), -500.0, FRAME_1)?;
    resize(&mut layout, after(Axis::Cols, 2), 1000.0, FRAME_1)?;
    let placed = drawn(&layout, FRAME_1)?;
    roomy(&placed, FRAME_1)?;
    assert_eq!(find(&placed, pane(Web))?, rect(478.0, 0.0, 396.0, 120.0));
    assert_eq!(
        find(&placed, pane(Remote))?,
        rect(478.0, 120.0, 396.0, 296.0)
    );
    assert_eq!(find(&placed, pane(Code))?, rect(874.0, 0.0, 160.0, 120.0));
    assert_eq!(find(&placed, pane(Chat))?, rect(874.0, 120.0, 160.0, 556.0));
    Ok(())
}

/// Plan default 6: a double-click gives Files and Changes of Review the same width; chat
/// keeps its own.
#[test]
fn a_double_click_evens_the_two_sides() -> Check {
    let mut layout = open(&REVIEW);
    even(&mut layout, after(Axis::Cols, 0), FRAME_1)?;
    let placed = drawn(&layout, FRAME_1)?;
    assert_eq!(find(&placed, pane(Files))?, rect(0.0, 0.0, 367.0, 676.0));
    assert_eq!(
        find(&placed, pane(Changes))?,
        rect(367.0, 0.0, 367.0, 526.0)
    );
    assert_eq!(
        find(&placed, pane(Terminal))?,
        rect(367.0, 526.0, 367.0, 150.0)
    );
    assert_eq!(find(&placed, pane(Chat))?, rect(734.0, 0.0, 300.0, 676.0));
    Ok(())
}

/// With web and the open file out, no pane ends on the first row line: it cannot be
/// dragged, and the line below it squeezes the grown panes down to one pane's minimum.
#[test]
fn a_line_no_pane_ends_on_is_not_there_to_drag() -> Check {
    let mut layout = open(&EVERYTHING);
    leave(&mut layout, pane(Web))?;
    leave(&mut layout, pane(Code))?;
    let gone = after(Axis::Rows, 0);
    assert_eq!(
        resize(&mut layout, gone, 10.0, FRAME_1),
        Err(GridError::NoSuchLine)
    );
    assert_eq!(
        line_at(
            &layout,
            FRAME_1,
            ChatSide::Right,
            Point { x: 600.0, y: 226.0 }
        ),
        None
    );
    resize(&mut layout, after(Axis::Rows, 1), -1000.0, FRAME_1)?;
    let placed = drawn(&layout, FRAME_1)?;
    let (remote, report) = (find(&placed, pane(Remote))?, find(&placed, pane(Report))?);
    assert!(near(remote, rect(478.0, 0.0, 286.0, 120.0)), "{remote:?}");
    assert!(near(report, rect(478.0, 120.0, 286.0, 556.0)), "{report:?}");
    Ok(())
}

/// Record 2.7.8: the pointer takes a line within three points of an edge of the pane under
/// it, columns before rows; a line through a pane and the grid's outer edge are not taken.
#[test]
fn the_pointer_finds_the_line_between_two_panes() {
    let layout = open(&EVERYTHING);
    let at = |side, x, y| line_at(&layout, FRAME_1, side, Point { x, y });
    let right = ChatSide::Right;
    assert_eq!(at(right, 480.0, 100.0), Some(after(Axis::Cols, 1)));
    assert_eq!(at(right, 475.5, 300.0), Some(after(Axis::Cols, 1)));
    assert_eq!(at(right, 600.0, 224.0), Some(after(Axis::Rows, 0)));
    assert_eq!(at(right, 600.0, 418.5), Some(after(Axis::Rows, 1)));
    assert_eq!(at(right, 765.0, 227.0), Some(after(Axis::Cols, 2)));
    assert_eq!(at(right, 300.0, 226.0), None, "Changes spans that line");
    assert_eq!(at(right, 600.0, 230.0), None, "four points off");
    assert_eq!(at(right, 1.0, 300.0), None, "the grid's own edge");
    assert_eq!(at(right, 600.0, 675.0), None, "the grid's own edge");
    assert_eq!(
        at(ChatSide::Left, 1034.0 - 480.0, 100.0),
        Some(after(Axis::Cols, 1)),
        "mirrored, the same line is at the mirrored place"
    );
}

/// Narrower than its fixed columns, Everything shares the shortfall: the flexible column
/// holds at the minimum, the fixed ones shrink together, and none goes under 160.
#[test]
fn a_narrow_grid_keeps_every_pane_at_the_minimum() -> Check {
    let size = Size { w: 700.0, h: 676.0 };
    let placed = drawn(&open(&EVERYTHING), size)?;
    roomy(&placed, size)?;
    assert_eq!(find(&placed, pane(Files))?.w, 160.0);
    assert_eq!(find(&placed, pane(Changes))?.w, 160.0);
    let (web, chat) = (find(&placed, pane(Web))?.w, find(&placed, pane(Chat))?.w);
    assert!(
        (web / chat - 286.0 / 270.0).abs() < 0.001,
        "{web} to {chat}"
    );
    Ok(())
}

/// A drag between the panes that are in leaves a pane that is out its share of the grid:
/// it had half the width when it left and has half when it is back.
#[test]
fn a_pane_that_is_out_keeps_its_share_through_a_drag() -> Check {
    let size = Size {
        w: 1000.0,
        h: 600.0,
    };
    let [left, middle, right] = [1, 2, 3].map(PaneId);
    let mut layout = Layout::single(left);
    split(&mut layout, left, Edge::Right, right)?;
    split(&mut layout, left, Edge::Right, middle)?;
    leave(&mut layout, right)?;
    resize(&mut layout, after(Axis::Cols, 0), 100.0, size)?;
    rejoin(&mut layout, right)?;
    let placed = drawn(&layout, size)?;
    let want = [
        (left, rect(0.0, 0.0, 300.0, 600.0)),
        (middle, rect(300.0, 0.0, 200.0, 600.0)),
        (right, rect(500.0, 0.0, 500.0, 600.0)),
    ];
    for (pane, rect) in want {
        assert!(near(find(&placed, pane)?, rect), "{pane:?}: {placed:?}");
    }
    Ok(())
}

/// Panes that are out keep their proportions in a room dragged while they are away: web
/// over the remote desktop was 226 to 190, and is again in the 340 the drag left them.
#[test]
fn panes_come_back_in_proportion_to_a_room_that_was_dragged() -> Check {
    let mut layout = open(&EVERYTHING);
    leave(&mut layout, pane(Web))?;
    leave(&mut layout, pane(Code))?;
    resize(&mut layout, after(Axis::Rows, 1), -76.0, FRAME_1)?;
    rejoin(&mut layout, pane(Web))?;
    rejoin(&mut layout, pane(Code))?;
    let placed = drawn(&layout, FRAME_1)?;
    let (web, remote) = (find(&placed, pane(Web))?.h, find(&placed, pane(Remote))?.h);
    assert!((web + remote - 340.0).abs() < 0.01, "{web} and {remote}");
    assert!(
        (web / remote - 226.0 / 190.0).abs() < 0.001,
        "{web} to {remote}"
    );
    assert_eq!(
        find(&placed, pane(Code))?.h,
        web,
        "the open file shares web's row"
    );
    Ok(())
}

/// Shares halve with every split, so they can get as small as numbers go. Two panes with
/// such shares still halve the grid, a drag between them takes, and no track is left unsound.
#[test]
fn shares_too_small_to_add_up_still_tile_the_grid() -> Check {
    let size = Size {
        w: 1000.0,
        h: 600.0,
    };
    let [left, middle, right] = [1, 2, 3].map(PaneId);
    let small = Track::Fr(1e-37);
    let mut layout = Layout {
        cols: vec![small, small, Track::Fr(1.0)],
        rows: vec![Track::Fr(1.0)],
        areas: vec![
            (left, area(1, 1, 2, 2)),
            (middle, area(1, 2, 2, 3)),
            (right, area(1, 3, 2, 4)),
        ],
        away: Vec::new(),
        stacks: Vec::new(),
    };
    leave(&mut layout, right)?;
    let placed = drawn(&layout, size)?;
    assert_eq!(find(&placed, left)?, rect(0.0, 0.0, 500.0, 600.0));
    resize(&mut layout, after(Axis::Cols, 0), 100.0, size)?;
    layout.check()?;
    let placed = drawn(&layout, size)?;
    assert!(
        near(find(&placed, left)?, rect(0.0, 0.0, 600.0, 600.0)),
        "{placed:?}"
    );
    rejoin(&mut layout, right)?;
    let placed = drawn(&layout, size)?;
    roomy(&placed, size)?;
    assert_eq!(placed.len(), 3);
    Ok(())
}

/// A file may hold a track as large as numbers go. Two of them under one pane still leave
/// the pane beside it its minimum, and every rectangle is a real one.
#[test]
fn tracks_too_large_to_add_up_still_tile_the_grid() -> Check {
    let size = Size {
        w: 1000.0,
        h: 600.0,
    };
    let (wide, narrow) = (PaneId(1), PaneId(2));
    let layout = Layout {
        cols: vec![Track::Px(f32::MAX), Track::Px(f32::MAX), Track::Fr(1.0)],
        rows: vec![Track::Fr(1.0)],
        areas: vec![(wide, area(1, 1, 2, 3)), (narrow, area(1, 3, 2, 4))],
        away: Vec::new(),
        stacks: Vec::new(),
    };
    layout.check()?;
    let placed = drawn(&layout, size)?;
    assert_eq!(find(&placed, wide)?, rect(0.0, 0.0, 840.0, 600.0));
    assert_eq!(find(&placed, narrow)?, rect(840.0, 0.0, 160.0, 600.0));
    Ok(())
}

/// With the flexible column out as windows the fixed ones stretch to fill the grid. A drag
/// between them is kept in their own sizes, so Changes comes back as wide as it left.
#[test]
fn the_flexible_column_comes_back_as_wide_as_it_left() -> Check {
    let mut layout = open(&EVERYTHING);
    leave(&mut layout, pane(Changes))?;
    leave(&mut layout, pane(Terminal))?;
    let before = drawn(&layout, FRAME_1)?;
    resize(&mut layout, after(Axis::Cols, 2), 50.0, FRAME_1)?;
    let placed = drawn(&layout, FRAME_1)?;
    let grown = find(&placed, pane(Web))?.w - find(&before, pane(Web))?.w;
    assert!((grown - 50.0).abs() < 0.01, "web grew by {grown}");
    assert!(near(
        find(&placed, pane(Files))?,
        find(&before, pane(Files))?
    ));
    rejoin(&mut layout, pane(Changes))?;
    rejoin(&mut layout, pane(Terminal))?;
    let placed = drawn(&layout, FRAME_1)?;
    let (files, changes) = (find(&placed, pane(Files))?, find(&placed, pane(Changes))?);
    assert!(near(files, rect(0.0, 0.0, 208.0, 676.0)), "{files:?}");
    assert!(near(changes, rect(208.0, 0.0, 270.0, 416.0)), "{changes:?}");
    Ok(())
}

/// A drag in a grid too narrow for its fixed columns moves the line there without making
/// the squeeze permanent: wide again, Files is 208 and the flexible column 270 as before.
#[test]
fn a_drag_in_a_narrow_grid_does_not_keep_it_squeezed() -> Check {
    let narrow = Size { w: 700.0, h: 676.0 };
    let mut layout = open(&EVERYTHING);
    let before = drawn(&layout, narrow)?;
    resize(&mut layout, after(Axis::Cols, 2), 10.0, narrow)?;
    let placed = drawn(&layout, narrow)?;
    let grown = find(&placed, pane(Web))?.w - find(&before, pane(Web))?.w;
    assert!((grown - 10.0).abs() < 0.01, "web grew by {grown}");
    assert!(near(
        find(&placed, pane(Files))?,
        find(&before, pane(Files))?
    ));
    let wide = drawn(&layout, FRAME_1)?;
    let (files, changes) = (find(&wide, pane(Files))?, find(&wide, pane(Changes))?);
    assert!(near(files, rect(0.0, 0.0, 208.0, 676.0)), "{files:?}");
    assert!(near(changes, rect(208.0, 0.0, 270.0, 416.0)), "{changes:?}");
    Ok(())
}

/// In a grid too narrow for it the flexible column is held at the minimum, and can still be
/// dragged wider. A flexible pane that is out keeps the share it had beside it.
#[test]
fn the_flexible_column_can_be_dragged_wider_in_a_narrow_grid() -> Check {
    let narrow = Size { w: 500.0, h: 600.0 };
    let [fixed, flexible, out] = [1, 2, 3].map(PaneId);
    let mut layout = Layout {
        cols: vec![Track::Px(600.0), Track::Fr(1.0), Track::Fr(1.0)],
        rows: vec![Track::Fr(1.0)],
        areas: vec![
            (fixed, area(1, 1, 2, 2)),
            (flexible, area(1, 2, 2, 3)),
            (out, area(1, 3, 2, 4)),
        ],
        away: Vec::new(),
        stacks: Vec::new(),
    };
    leave(&mut layout, out)?;
    let placed = drawn(&layout, narrow)?;
    assert_eq!(find(&placed, flexible)?, rect(340.0, 0.0, 160.0, 600.0));
    resize(&mut layout, after(Axis::Cols, 0), -100.0, narrow)?;
    let placed = drawn(&layout, narrow)?;
    let dragged = find(&placed, flexible)?;
    assert!(near(dragged, rect(240.0, 0.0, 260.0, 600.0)), "{dragged:?}");
    rejoin(&mut layout, out)?;
    let size = Size {
        w: 1000.0,
        h: 600.0,
    };
    let placed = drawn(&layout, size)?;
    let (kept, back) = (find(&placed, flexible)?.w, find(&placed, out)?.w);
    assert!((kept + back - 760.0).abs() < 0.01, "{kept} and {back}");
    assert!(
        (kept / back - 260.0 / 160.0).abs() < 0.001,
        "{kept} to {back}"
    );
    Ok(())
}

/// The same drag where the share in view is as small as numbers go: the share that is out
/// would follow it past the largest number, and is held there, a track a file can hold.
#[test]
fn a_share_that_is_out_never_outgrows_what_a_file_holds() -> Check {
    let narrow = Size { w: 500.0, h: 600.0 };
    let [fixed, flexible, out] = [1, 2, 3].map(PaneId);
    let mut layout = Layout {
        cols: vec![Track::Px(600.0), Track::Fr(1e-37), Track::Fr(1.0)],
        rows: vec![Track::Fr(1.0)],
        areas: vec![
            (fixed, area(1, 1, 2, 2)),
            (flexible, area(1, 2, 2, 3)),
            (out, area(1, 3, 2, 4)),
        ],
        away: Vec::new(),
        stacks: Vec::new(),
    };
    leave(&mut layout, out)?;
    resize(&mut layout, after(Axis::Cols, 0), -100.0, narrow)?;
    layout.check()?;
    let dragged = find(&drawn(&layout, narrow)?, flexible)?;
    assert!(near(dragged, rect(240.0, 0.0, 260.0, 600.0)), "{dragged:?}");
    rejoin(&mut layout, out)?;
    assert_eq!(drawn(&layout, narrow)?.len(), 3);
    Ok(())
}
