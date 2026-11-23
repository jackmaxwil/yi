//! A thing dropped on the grid: the pane about it lands where a carried pane would, a tab on
//! a pane's middle, a half on its edge, a column or a row on the window's.

use crate::grid::layout::{Edge, Point, Rect, Size};
use crate::grid::place::probe::{Check, FRAME_1, find};
use crate::grid::swap::{DOCKED, Spot};
use crate::ids::PaneId;
use crate::model::Model;
use crate::pane::PaneKind::{self, Changes, Code, Files, Terminal};

use super::GridMsg;
use super::beside::file;
use super::stacks::{open, window};
use super::tests::{MAIN, change, drawn, pane};

/// Chat, Changes beside it, Files in the side column and a shell along the bottom.
fn four() -> Model {
    let mut model = window("a");
    for kind in [Changes, Files, Terminal] {
        open(&mut model, kind);
    }
    model
}

/// Drops the file at `path` on `at`, and gives back the panes about it.
fn put(model: &mut Model, path: &str, at: Spot) -> Result<Vec<PaneId>, String> {
    let (window, kind, thing, size) = (MAIN, Code, file(path)?, FRAME_1);
    let dropped = GridMsg::Put {
        window,
        kind,
        thing: thing.clone(),
        at,
        size,
    };
    change(model, dropped);
    let about = |pane: &&crate::grid::windows::Pane| pane.thing.as_ref() == Some(&thing);
    Ok(model
        .grid
        .panes
        .iter()
        .filter(about)
        .map(|pane| pane.id)
        .collect())
}

fn rect(model: &Model, pane: PaneId) -> Result<Rect, String> {
    find(&drawn(model)?, pane)
}

fn tabs(model: &Model, pane: PaneId) -> Vec<Option<PaneKind>> {
    let tabs = model.grid.tabs(MAIN, pane);
    tabs.iter().map(|pane| model.grid.kind(*pane)).collect()
}

/// On a pane's middle the file is the tab in front of that pane's slot, whatever place the
/// slot is: no slot is made, and no other pane moves.
#[test]
fn a_file_dropped_on_a_panes_middle_is_the_tab_in_front_of_its_slot() -> Check {
    let mut model = four();
    let (shell, before) = (pane(&model, Terminal)?, drawn(&model)?);
    let made = put(&mut model, "src/a.rs", Spot::Onto(shell))?;
    let [code] = made[..] else {
        return Err(format!("{} panes about the file", made.len()).into());
    };
    assert_eq!(tabs(&model, code), [Some(Terminal), Some(Code)]);
    let after = drawn(&model)?;
    assert_eq!(after.len(), before.len(), "no slot more");
    assert_eq!(
        rect(&model, code)?,
        find(&before, shell)?,
        "the shell's slot"
    );
    let changes = pane(&model, Changes)?;
    assert_eq!(find(&after, changes)?, find(&before, changes)?);
    assert_eq!(tabs(&model, changes), [Some(Changes)], "work took no tab");
    Ok(())
}

/// With no work pane open the file has no tabs to join in its own place, and still it is
/// a tab of the pane it is dropped on: no slot is made for it and that pane stays put.
#[test]
fn a_file_with_no_tabs_of_its_own_still_joins_the_pane_it_is_dropped_on() -> Check {
    let mut model = window("a");
    for kind in [Files, Terminal] {
        open(&mut model, kind);
    }
    let (shell, before) = (pane(&model, Terminal)?, drawn(&model)?);
    let made = put(&mut model, "src/a.rs", Spot::Onto(shell))?;
    let [code] = made[..] else {
        return Err(format!("{} panes about the file", made.len()).into());
    };
    assert_eq!(tabs(&model, code), [Some(Terminal), Some(Code)]);
    let after = drawn(&model)?;
    assert_eq!(after.len(), before.len(), "no slot more");
    assert_eq!(
        rect(&model, code)?,
        find(&before, shell)?,
        "the shell's slot"
    );
    Ok(())
}

/// On the tabs of its own place it joins them in front; on their edge it takes that half.
#[test]
fn a_file_dropped_on_the_work_tabs_joins_them_or_takes_their_edge() -> Check {
    let mut model = four();
    let changes = pane(&model, Changes)?;
    let slot = rect(&model, changes)?;
    let made = put(&mut model, "src/a.rs", Spot::Onto(changes))?;
    let [code] = made[..] else {
        return Err(format!("{} panes about the file", made.len()).into());
    };
    assert_eq!(tabs(&model, code), [Some(Changes), Some(Code)]);
    assert_eq!(rect(&model, code)?, slot, "in front, in the slot");
    let made = put(&mut model, "src/b.rs", Spot::Beside(code, Edge::Right))?;
    let [other] = made[..] else {
        return Err(format!("{} panes about the second file", made.len()).into());
    };
    let (left, right) = (rect(&model, code)?, rect(&model, other)?);
    assert_eq!(tabs(&model, other), [Some(Code)], "alone in its half");
    assert_eq!(tabs(&model, code), [Some(Changes), Some(Code)]);
    assert!((left.x - slot.x).abs() < 0.5 && (right.x - (left.x + left.w)).abs() < 1.5);
    assert!(
        (right.x + right.w - (slot.x + slot.w)).abs() < 0.5,
        "{right:?}"
    );
    Ok(())
}

/// On a pane's edge it takes the half of that pane's slot on that edge; on the window's, a
/// row or a column across it.
#[test]
fn a_file_dropped_on_an_edge_takes_that_half_or_a_track_of_the_window() -> Check {
    let mut model = four();
    let changes = pane(&model, Changes)?;
    let slot = rect(&model, changes)?;
    let made = put(&mut model, "src/a.rs", Spot::Beside(changes, Edge::Bottom))?;
    let [code] = made[..] else {
        return Err(format!("{} panes about the file", made.len()).into());
    };
    let (upper, lower) = (rect(&model, changes)?, rect(&model, code)?);
    assert_eq!(
        (upper.x, upper.w, lower.x, lower.w),
        (slot.x, slot.w, slot.x, slot.w)
    );
    assert!(lower.y > upper.y && (lower.y + lower.h - (slot.y + slot.h)).abs() < 0.5);
    let made = put(&mut model, "src/b.rs", Spot::Side(Edge::Right))?;
    let [column] = made[..] else {
        return Err(format!("{} panes about the second file", made.len()).into());
    };
    let column = rect(&model, column)?;
    assert_eq!((column.h, column.w), (FRAME_1.h, DOCKED.w), "a column");
    assert!(
        (column.x + column.w - FRAME_1.w).abs() < 0.5,
        "on the right"
    );
    Ok(())
}

/// A file with a pane already is not shown twice: its pane goes where it is dropped.
#[test]
fn a_file_dropped_again_moves_its_pane_and_makes_no_second() -> Check {
    let mut model = four();
    let (changes, shell) = (pane(&model, Changes)?, pane(&model, Terminal)?);
    let first = put(&mut model, "src/a.rs", Spot::Onto(changes))?;
    let again = put(&mut model, "src/a.rs", Spot::Onto(shell))?;
    assert_eq!(first, again, "the same one pane");
    assert_eq!(tabs(&model, shell), [Some(Terminal), Some(Code)]);
    assert_eq!(tabs(&model, changes), [Some(Changes)], "and it left work");
    Ok(())
}

/// The zone says where the pane would be drawn and changes nothing. The pane the file came
/// from is no target; a drop with no room is refused and changes nothing either.
#[test]
fn the_zone_says_where_it_lands_and_a_refused_drop_changes_nothing() -> Check {
    let mut model = four();
    let (files, shell) = (pane(&model, Files)?, pane(&model, Terminal)?);
    let thing = file("src/a.rs")?;
    let middle = |rect: Rect| Point {
        x: rect.x + rect.w / 2.0,
        y: rect.y + rect.h / 2.0,
    };
    let zone = |model: &Model, size, at| {
        let grid = &model.grid;
        grid.zone_for(MAIN, size, (Code, &thing), files, at)
    };
    let at = rect(&model, shell)?;
    let over = zone(&model, FRAME_1, middle(at)).ok_or("no zone over the shell")?;
    assert_eq!((over.spot, over.lands), (Spot::Onto(shell), Some(at)));
    let home = rect(&model, files)?;
    assert_eq!(zone(&model, FRAME_1, middle(home)), None, "its own pane");
    let edge = Point {
        x: home.x + 20.0,
        ..middle(home)
    };
    assert_eq!(zone(&model, FRAME_1, edge), None, "nor an edge of it");
    let outside = Point { x: -4.0, y: 40.0 };
    assert_eq!(zone(&model, FRAME_1, outside), None, "off the grid");
    // A window this small has no room for one more column of the least width.
    let small = Size { w: 500.0, h: 400.0 };
    let side = Point { x: 496.0, y: 200.0 };
    let tight = zone(&model, small, side).ok_or("no zone at the edge")?;
    assert_eq!((tight.spot, tight.lands), (Spot::Side(Edge::Right), None));
    let before = model.grid.clone();
    let (window, kind, at) = (MAIN, Code, tight.spot);
    let refused = GridMsg::Put {
        window,
        kind,
        thing: thing.clone(),
        at,
        size: small,
    };
    change(&mut model, refused);
    assert_eq!(model.grid, before, "refused whole");
    change(&mut model, GridMsg::Fill(shell));
    assert_eq!(
        zone(&model, FRAME_1, middle(at_rest(&before)?)),
        None,
        "filled"
    );
    Ok(())
}

/// A pane out in a window of its own keeps its tabs where it came from: a file let go on
/// its middle is no tab of it, and nothing changes.
#[test]
fn a_file_dropped_on_a_pane_out_of_its_window_is_refused() -> Check {
    let mut model = four();
    let shell = pane(&model, Terminal)?;
    change(&mut model, GridMsg::Leave(shell));
    let window = model
        .grid
        .window_of(shell)
        .ok_or("the shell is in no window")?;
    assert_ne!(window, MAIN, "it has a window of its own");
    let before = model.grid.clone();
    let (kind, thing, at, size) = (Code, file("src/a.rs")?, Spot::Onto(shell), FRAME_1);
    let dropped = GridMsg::Put {
        window,
        kind,
        thing,
        at,
        size,
    };
    change(&mut model, dropped);
    assert_eq!(model.grid, before);
    Ok(())
}

/// The middle of the chat as `grid` draws it.
fn at_rest(grid: &crate::grid::windows::Windows) -> Result<Rect, String> {
    let placed = grid.place(MAIN, FRAME_1);
    let chat = placed
        .iter()
        .find(|(pane, _)| grid.kind(*pane) == Some(PaneKind::Chat));
    chat.map(|(_, rect)| *rect)
        .ok_or_else(|| "no chat".to_owned())
}
