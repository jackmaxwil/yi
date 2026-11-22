//! A pane moved to a place by the person: it joins that place's stack in front, or opens the
//! place, and is of that place from then on.

use crate::grid::place::probe::{Check, FRAME_1};
use crate::grid::windows::Windows;
use crate::ids::PaneId;
use crate::model::Model;
use crate::pane::PaneKind::{Changes, Chat, Code, Debugger, Files, IPython, Terminal, Web};
use crate::pane::Place;

use super::GridMsg;
use super::tests::{self, MAIN, change, drawn, have, pane};

fn showing(saved: &'static str) -> Model {
    let mut model = tests::showing(saved);
    // Every kind the tests move has something to show, so none rests out of the grid.
    have(&mut model, MAIN, "a");
    model
}

fn is_drawn(model: &Model, pane: PaneId) -> Result<bool, String> {
    Ok(drawn(model)?.iter().any(|(id, _)| *id == pane))
}

fn to(model: &mut Model, pane: PaneId, place: Place) {
    change(model, GridMsg::Place { pane, place });
}

#[test]
fn a_pane_moved_to_a_place_is_a_tab_in_front_of_that_places_stack() -> Check {
    let mut model = showing("everything");
    let files = pane(&model, Files)?;
    let slots = drawn(&model)?.len();
    assert_eq!(model.grid.slot_place(files), Some(Place::Side));
    assert_eq!(model.grid.kept(files), None, "seated by the layout");
    to(&mut model, files, Place::Work);
    let tabs = model.grid.tabs(MAIN, files);
    let [host, last] = tabs[..] else {
        return Err(format!("the tabs of its slot: {tabs:?}").into());
    };
    assert_eq!(last, files, "behind the host in tab order");
    let of_work = model.grid.kind(host).map(|kind| kind.place());
    assert_eq!(of_work, Some(Place::Work), "the stack of a work pane");
    assert!(
        is_drawn(&model, files)? && !is_drawn(&model, host)?,
        "in front"
    );
    assert_eq!(drawn(&model)?.len(), slots - 1, "the slot it left is gone");
    assert_eq!(model.grid.slot_place(files), Some(Place::Work));
    assert_eq!(model.grid.kept(files), Some(false), "the person's now");
    // Alone in the one slot of its place, a pane moved there again would close the slot and
    // open it anew at the edge.
    let terminal = pane(&model, Terminal)?;
    let before = model.grid.clone();
    to(&mut model, files, Place::Work);
    to(&mut model, terminal, Place::Shelf);
    assert_eq!(model.grid, before, "there already, nothing moves");
    Ok(())
}

/// Moved where no pane of that place is, the pane opens the place: the shelf is a row along
/// the bottom. It is the shelf's then, whatever its kind, and the next one moved joins it.
#[test]
fn a_pane_moved_to_an_empty_place_opens_it_and_the_next_joins_its_tabs() -> Check {
    let mut model = showing("everything");
    let (terminal, files) = (pane(&model, Terminal)?, pane(&model, Files)?);
    let (code, web) = (pane(&model, Code)?, pane(&model, Web)?);
    to(&mut model, terminal, Place::Side);
    assert_eq!(model.grid.tabs(MAIN, files), [files, terminal]);
    let shelf = |grid: &Windows| grid.panes.iter().any(|p| grid.place_of(p) == Place::Shelf);
    assert!(!shelf(&model.grid), "the shelf is empty");
    to(&mut model, code, Place::Shelf);
    let placed = drawn(&model)?;
    let found = placed.iter().find(|(id, _)| *id == code);
    let (_, rect) = found.ok_or("Code is not drawn")?;
    let along = rect.x.abs() < 0.5 && (rect.w - FRAME_1.w).abs() < 0.5;
    let bottom = (rect.y + rect.h - FRAME_1.h).abs() < 0.5;
    assert!(along && bottom, "{rect:?} is not a row along the bottom");
    assert_eq!(model.grid.slot_place(code), Some(Place::Shelf));
    to(&mut model, web, Place::Shelf);
    assert_eq!(model.grid.tabs(MAIN, web), [code, web]);
    assert!(is_drawn(&model, web)?, "the one just moved is in front");
    Ok(())
}

/// A tab moved away leaves its slot to the tab after it; the last pane of a window moves
/// nowhere, and the window is as it was.
#[test]
fn a_tab_moved_away_leaves_its_slot_and_a_last_pane_stays() -> Check {
    let mut model = showing("everything");
    let (window, kind) = (MAIN, Debugger);
    change(&mut model, GridMsg::Open { window, kind });
    let (changes, debugger) = (pane(&model, Changes)?, pane(&model, Debugger)?);
    assert!(is_drawn(&model, debugger)? && !is_drawn(&model, changes)?);
    to(&mut model, debugger, Place::Side);
    assert!(is_drawn(&model, changes)?, "Changes has the slot again");
    assert_eq!(model.grid.tabs(MAIN, changes), [changes]);
    assert_eq!(model.grid.slot_place(debugger), Some(Place::Side));
    let mut alone = showing("just-chat");
    let (chat, before) = (pane(&alone, Chat)?, alone.grid.clone());
    to(&mut alone, chat, Place::Work);
    assert_eq!(alone.grid, before, "the one pane has nowhere to go");
    Ok(())
}

#[test]
fn the_place_a_pane_was_moved_to_is_kept_in_the_grid_file() -> Check {
    let mut model = showing("everything");
    let files = pane(&model, Files)?;
    to(&mut model, files, Place::Work);
    let text = serde_json::to_string(&model.grid)?;
    assert!(text.contains(r#""at":"work""#), "{text}");
    let read: Windows = serde_json::from_str(&text)?;
    let at = |grid: &Windows| grid.panes.iter().find(|p| p.id == files).map(|p| p.at);
    assert_eq!(at(&read), Some(Some(Place::Work)));
    let odd = text.replace(r#""at":"work""#, r#""at":"attic""#);
    let read: Windows = serde_json::from_str(&odd)?;
    assert_eq!(
        at(&read),
        Some(None),
        "a word that names no place is no move"
    );
    Ok(())
}

/// The tab after the one moved may be another session's: the slot it left goes to the first
/// tab that shows in the session shown.
#[test]
fn the_slot_a_moved_tab_left_shows_a_tab_of_the_session_shown() -> Check {
    let mut model = showing("everything");
    let window = MAIN;
    let open = |model: &mut Model, kind| change(model, GridMsg::Open { window, kind });
    open(&mut model, Debugger);
    have(&mut model, window, "b");
    open(&mut model, IPython);
    let (changes, debugger) = (pane(&model, Changes)?, pane(&model, Debugger)?);
    let ipython = pane(&model, IPython)?;
    assert_eq!(model.grid.tabs(MAIN, changes), [changes, ipython]);
    change(&mut model, GridMsg::Front(changes));
    to(&mut model, changes, Place::Side);
    assert!(is_drawn(&model, ipython)?, "the tab of this session has it");
    assert!(!is_drawn(&model, debugger)?, "not the other session's tab");
    Ok(())
}
