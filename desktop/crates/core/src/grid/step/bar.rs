//! What a pane bar's right-click rows ask of the grid: a second pane on a side, whether a
//! pane is kept, and which tabs closing the others closes.

use crate::command::by_kind::{TAB_CLOSE_OTHERS, TAB_KEEP, TAB_KEEP_ACROSS, keeps};
use crate::grid::layout::{Edge, Rect};
use crate::grid::place::probe::{Check, FRAME_1};
use crate::grid::swap::Spot;
use crate::ids::{PaneId, ThreadId};
use crate::model::Model;
use crate::pane::PaneKind::{self, Changes, Chat, Code, Debugger};

use super::GridMsg;
use super::tests::{MAIN, change, drawn, pane, record};

fn everything() -> Model {
    let mut model = record();
    let (window, saved) = (MAIN, "everything");
    change(&mut model, GridMsg::Show { window, saved });
    let (kinds, thread) = (vec![Chat, Code, Changes], Some(ThreadId::new("a")));
    change(
        &mut model,
        GridMsg::Have {
            window,
            kinds,
            thread,
        },
    );
    model
}

fn rect(model: &Model, pane: PaneId) -> Result<Rect, String> {
    let found = drawn(model)?.into_iter().find(|(id, _)| *id == pane);
    found
        .map(|(_, rect)| rect)
        .ok_or(format!("{pane:?} is not drawn"))
}

/// The pane a change made: the one of `kind` the grid did not hold `before`.
fn made(model: &Model, before: &[PaneId], kind: PaneKind) -> Result<PaneId, String> {
    let new = |pane: &&crate::grid::windows::Pane| !before.contains(&pane.id) && pane.kind == kind;
    let found = model.grid.panes.iter().find(new);
    found.map(|pane| pane.id).ok_or(format!("no new {kind:?}"))
}

fn ids(model: &Model) -> Vec<PaneId> {
    model.grid.panes.iter().map(|pane| pane.id).collect()
}

#[test]
fn a_split_down_puts_a_second_pane_of_the_kind_in_the_lower_half_of_the_slot() -> Check {
    let mut model = everything();
    let code = pane(&model, Code)?;
    let (was, before) = (rect(&model, code)?, ids(&model));
    let edge = Edge::Bottom;
    change(&mut model, GridMsg::SplitAt { pane: code, edge });
    let copy = made(&model, &before, Code)?;
    let (top, bottom) = (rect(&model, code)?, rect(&model, copy)?);
    assert_eq!(
        (top.x, top.y, top.w),
        (was.x, was.y, was.w),
        "the pane stays"
    );
    assert_eq!((bottom.x, bottom.w), (was.x, was.w), "the copy is under it");
    let under = (bottom.y - (top.y + top.h)).abs() < 0.5;
    assert!(under, "{bottom:?} is not right under {top:?}");
    Ok(())
}

/// Said of a tab that waits behind another, the copy is cut from the slot the two share.
#[test]
fn a_split_said_of_a_tab_behind_cuts_the_slot_it_waits_in() -> Check {
    let mut model = everything();
    let (window, kind) = (MAIN, Debugger);
    change(&mut model, GridMsg::Open { window, kind });
    let (changes, debugger) = (pane(&model, Changes)?, pane(&model, Debugger)?);
    assert_eq!(model.grid.tabs(MAIN, debugger), [changes, debugger]);
    assert!(rect(&model, changes).is_err(), "Changes waits behind");
    let (was, before) = (rect(&model, debugger)?, ids(&model));
    let (pane, edge) = (changes, Edge::Bottom);
    change(&mut model, GridMsg::SplitAt { pane, edge });
    let copy = made(&model, &before, Changes)?;
    let (top, bottom) = (rect(&model, debugger)?, rect(&model, copy)?);
    assert_eq!((top.x, top.y, bottom.x), (was.x, was.y, was.x));
    let under = (bottom.y - (top.y + top.h)).abs() < 0.5;
    assert!(under, "{bottom:?} is not right under {top:?}");
    assert_eq!(model.grid.tabs(MAIN, debugger), [changes, debugger]);
    Ok(())
}

/// A pane a layout seated is not kept; brought in front by the person it is kept in the
/// session shown, and kept across sessions it is the window's. Each row ticks for its own.
#[test]
fn a_pane_is_kept_here_or_across_sessions_and_a_seated_one_is_neither() -> Check {
    let mut model = everything();
    let changes = pane(&model, Changes)?;
    let ticks = |model: &Model| {
        let grid = &model.grid;
        (
            keeps(grid, TAB_KEEP, changes),
            keeps(grid, TAB_KEEP_ACROSS, changes),
        )
    };
    assert_eq!(model.grid.kept(changes), None, "seated by the layout");
    assert_eq!(ticks(&model), (Some(false), Some(false)));
    change(&mut model, GridMsg::Front(changes));
    assert_eq!(model.grid.kept(changes), Some(false), "kept here");
    assert_eq!(ticks(&model), (Some(true), Some(false)));
    let (pane, on) = (changes, true);
    change(&mut model, GridMsg::Across { pane, on });
    assert_eq!(model.grid.kept(changes), Some(true), "kept across sessions");
    assert_eq!(ticks(&model), (Some(false), Some(true)));
    assert_eq!(keeps(&model.grid, TAB_CLOSE_OTHERS, changes), None);
    Ok(())
}

/// Closing the others closes the other tabs of the slot that show here, and never the
/// window's own chat.
#[test]
fn the_mates_of_a_pane_are_the_other_tabs_of_its_slot_but_never_the_chat() -> Check {
    let mut model = everything();
    let (window, kind) = (MAIN, Debugger);
    change(&mut model, GridMsg::Open { window, kind });
    let (changes, debugger) = (pane(&model, Changes)?, pane(&model, Debugger)?);
    let (chat, code) = (pane(&model, Chat)?, pane(&model, Code)?);
    assert_eq!(model.grid.mates(debugger), [changes]);
    assert_eq!(model.grid.mates(changes), [debugger]);
    assert_eq!(model.grid.mates(code), [], "alone in its slot");
    let (pane, at, size) = (debugger, Spot::Onto(chat), FRAME_1);
    change(
        &mut model,
        GridMsg::Drop {
            window,
            pane,
            at,
            size,
        },
    );
    assert_eq!(model.grid.tabs(MAIN, chat), [chat, debugger]);
    assert_eq!(model.grid.mates(debugger), [], "the chat is spared");
    assert_eq!(model.grid.mates(chat), [debugger]);
    Ok(())
}
