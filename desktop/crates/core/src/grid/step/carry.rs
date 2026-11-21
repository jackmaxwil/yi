//! Carrying a pane that is one tab among others: only it goes, and the tabs behind it stay.

use crate::grid::layout::{Edge, Point};
use crate::grid::place::probe::{Check, FRAME_1, find};
use crate::grid::swap::Spot;
use crate::model::Model;
use crate::pane::PaneKind::{self, Changes, Chat, Code, Terminal, Web};

use super::GridMsg;
use super::stacks::{kinds, open, show, tabs, window};
use super::tests::{MAIN, change, drawn, pane};

fn carry(model: &mut Model, kind: PaneKind, at: Spot) -> Check {
    let (window, pane, size) = (MAIN, pane(model, kind)?, FRAME_1);
    let dropped = GridMsg::Drop {
        window,
        pane,
        at,
        size,
    };
    change(model, dropped);
    Ok(())
}

fn sorted(mut kinds: Vec<PaneKind>) -> Vec<PaneKind> {
    kinds.sort();
    kinds
}

/// A tab dropped along the window's foot goes there alone. The slot it left keeps its other
/// tabs in the same column, with the tab just before it in front.
#[test]
fn a_tab_dropped_on_an_edge_goes_alone_and_its_neighbour_has_the_slot() -> Check {
    let mut model = window("a");
    for kind in [Changes, Code, Web] {
        open(&mut model, kind);
    }
    let slot = find(&drawn(&model)?, pane(&model, Web)?)?;
    carry(&mut model, Web, Spot::Side(Edge::Bottom))?;
    assert_eq!(kinds(&model)?, sorted(vec![Chat, Code, Web]));
    assert_eq!(tabs(&model, Code)?, [Changes, Code], "the rest stayed");
    assert_eq!(tabs(&model, Web)?, [Web], "it went alone");
    let placed = drawn(&model)?;
    let (left, foot) = (
        find(&placed, pane(&model, Code)?)?,
        find(&placed, pane(&model, Web)?)?,
    );
    assert_eq!((left.x, left.w), (slot.x, slot.w), "in the slot it left");
    assert_eq!(foot.w, FRAME_1.w, "a row across the window");
    Ok(())
}

/// An edge of its own slot cuts a tab from its tabs: it takes that half and the next tab
/// the other. The middle of its own slot is no place to drop, and a pane alone has neither.
#[test]
fn a_tab_dropped_on_an_edge_of_its_own_slot_is_cut_from_its_tabs() -> Check {
    let mut model = window("a");
    open(&mut model, Changes);
    let lone = find(&drawn(&model)?, pane(&model, Changes)?)?;
    let (changes, left) = (pane(&model, Changes)?, lone.x + 2.0);
    let at = |x: f32| Point {
        x,
        y: lone.y + lone.h / 2.0,
    };
    let zone = |model: &Model, pane, x| model.grid.zone_at(MAIN, FRAME_1, pane, at(x));
    // The slot is cut from the chat's right half: its left edge is no edge of the window.
    assert_eq!(zone(&model, changes, left), None, "alone, it has no tabs");
    open(&mut model, Code);
    let code = pane(&model, Code)?;
    let spot = zone(&model, code, left).map(|zone| zone.spot);
    assert_eq!(spot, Some(Spot::Beside(code, Edge::Left)));
    assert_eq!(
        zone(&model, code, lone.x + lone.w / 2.0),
        None,
        "its middle"
    );
    carry(&mut model, Code, Spot::Beside(code, Edge::Left))?;
    let placed = drawn(&model)?;
    let (kept, cut) = (find(&placed, changes)?, find(&placed, code)?);
    assert_eq!(cut.x, lone.x, "it has the left half");
    assert_eq!(kept.x + kept.w, lone.x + lone.w, "the next tab the right");
    assert_eq!(cut.x + cut.w, kept.x, "of the one slot");
    assert_eq!(tabs(&model, Changes)?, [Changes]);
    Ok(())
}

/// A tab dropped on another pane's middle joins that pane's tabs, in front, and the window
/// shows a kept layout no longer. A pane alone in its slot still swaps places.
#[test]
fn a_tab_dropped_on_a_middle_joins_those_tabs_and_a_lone_pane_swaps() -> Check {
    let mut model = window("a");
    for kind in [Changes, Code, Terminal] {
        open(&mut model, kind);
    }
    let (window, name) = (MAIN, "mine".to_owned());
    change(&mut model, GridMsg::Keep { window, name });
    let kept = |model: &Model| model.grid.window(MAIN).and_then(|window| window.own);
    assert!(kept(&model).is_some());
    let placed = drawn(&model)?;
    let (code, terminal) = (pane(&model, Code)?, pane(&model, Terminal)?);
    let (work, shelf) = (find(&placed, code)?, find(&placed, terminal)?);
    carry(&mut model, Code, Spot::Onto(terminal))?;
    assert_eq!(tabs(&model, Code)?, [Terminal, Code]);
    assert_eq!(tabs(&model, Changes)?, [Changes]);
    let placed = drawn(&model)?;
    assert_eq!(find(&placed, code)?, shelf, "in front, on the shelf");
    assert_eq!(find(&placed, pane(&model, Changes)?)?, work);
    assert_eq!(
        kept(&model),
        None,
        "a tab moved is the kept layout no longer"
    );
    carry(&mut model, Changes, Spot::Onto(code))?;
    let placed = drawn(&model)?;
    assert_eq!(find(&placed, pane(&model, Changes)?)?, shelf, "swapped");
    assert_eq!(find(&placed, code)?, work);
    assert_eq!(tabs(&model, Code)?, [Terminal, Code], "with its tabs");
    Ok(())
}

/// The tab left with the slot may be open in another session only: the slot then closes up
/// here, and shows that tab again in its own session.
#[test]
fn a_slot_left_to_another_sessions_tab_closes_up() -> Check {
    let mut model = window("a");
    open(&mut model, Changes);
    open(&mut model, Web);
    show(&mut model, "b");
    open(&mut model, Web);
    assert_eq!(kinds(&model)?, sorted(vec![Chat, Web]));
    carry(&mut model, Web, Spot::Side(Edge::Bottom))?;
    assert_eq!(
        kinds(&model)?,
        sorted(vec![Chat, Web]),
        "Changes is not b's"
    );
    assert_eq!(tabs(&model, Web)?, [Web]);
    show(&mut model, "a");
    assert_eq!(kinds(&model)?, sorted(vec![Chat, Changes, Web]));
    Ok(())
}

/// A pane out of another window keeps its tabs where it came from: a tab dropped on its
/// middle is refused and no zone is offered there.
#[test]
fn a_tab_does_not_join_a_pane_that_is_out_of_another_window() -> Check {
    let mut model = window("a");
    open(&mut model, Terminal);
    let terminal = pane(&model, Terminal)?;
    change(&mut model, GridMsg::Leave(terminal));
    let window = model.grid.window_of(terminal).ok_or("it is in no window")?;
    assert_ne!(window, MAIN);
    for kind in [Changes, Code] {
        change(&mut model, GridMsg::Open { window, kind });
    }
    let code = model.grid.place(window, FRAME_1);
    let code = code
        .iter()
        .map(|(pane, _)| *pane)
        .find(|pane| *pane != terminal);
    let code = code.ok_or("no tab was drawn beside the visitor")?;
    assert_eq!(model.grid.tabs(window, code).len(), 2);
    let before = model.grid.clone();
    let (pane, at, size) = (code, Spot::Onto(terminal), FRAME_1);
    let dropped = GridMsg::Drop {
        window,
        pane,
        at,
        size,
    };
    change(&mut model, dropped);
    assert_eq!(model.grid, before);
    let over = find(&model.grid.place(window, FRAME_1), terminal)?;
    let middle = Point {
        x: over.x + over.w / 2.0,
        y: over.y + over.h / 2.0,
    };
    assert_eq!(model.grid.zone_at(window, FRAME_1, code, middle), None);
    Ok(())
}
