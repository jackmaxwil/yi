//! A slot holds a stack of panes as tabs: a pane opens in the stack of its place, one tab is
//! drawn, and the tabs follow the session shown.

use crate::grid::layout::{MIN_PANE, Size};
use crate::grid::place::probe::{Check, FRAME_1, find};
use crate::grid::swap::DOCKED;
use crate::grid::windows::Windows;
use crate::ids::ThreadId;
use crate::model::Model;
use crate::pane::PaneKind::{self, Agents, Changes, Chat, Code, Files, Terminal, Trace, Web};

use super::GridMsg;
use super::tests::{MAIN, change, drawn, kinds, open, pane, record, reread};

/// The chat alone, in a window showing session `thread`.
pub(super) fn window(thread: &str) -> Model {
    let mut model = record();
    let (window, saved) = (MAIN, "chat");
    change(&mut model, GridMsg::Show { window, saved });
    show(&mut model, thread);
    model
}

pub(super) fn show(model: &mut Model, thread: &str) {
    let thread = (!thread.is_empty()).then(|| ThreadId::new(thread));
    // Every kind but those a rule opens by itself: these tests open their own panes.
    let kinds = PaneKind::ALL.iter().copied();
    let (window, kinds) = (MAIN, kinds.filter(|kind| !kind.arrives()).collect());
    let told = GridMsg::Have {
        window,
        kinds,
        thread,
    };
    change(model, told);
}

pub(super) fn tabs(model: &Model, of: PaneKind) -> Result<Vec<PaneKind>, String> {
    let tabs = model.grid.tabs(MAIN, pane(model, of)?);
    let kind = |pane: &_| model.grid.kind(*pane).ok_or("a tab of no kind".to_owned());
    tabs.iter().map(kind).collect()
}

/// Work opens one slot beside the chat and every later work pane is a tab in it: three
/// panes asked for, two slots drawn. A tab brought in front is drawn in the same place.
#[test]
fn panes_of_one_place_share_a_slot_as_tabs() -> Check {
    let mut model = window("a");
    open(&mut model, Changes);
    let slot = find(&drawn(&model)?, pane(&model, Changes)?)?;
    open(&mut model, Code);
    open(&mut model, Web);
    assert_eq!(
        kinds(&model)?,
        [Chat, Web],
        "the last asked for is in front"
    );
    assert_eq!(
        tabs(&model, Web)?,
        [Changes, Code, Web],
        "in the order asked"
    );
    assert_eq!(find(&drawn(&model)?, pane(&model, Web)?)?, slot);
    let changes = pane(&model, Changes)?;
    change(&mut model, GridMsg::Front(changes));
    assert_eq!(kinds(&model)?, [Chat, Changes]);
    assert_eq!(find(&drawn(&model)?, pane(&model, Changes)?)?, slot);
    open(&mut model, Code);
    assert_eq!(
        kinds(&model)?,
        [Chat, Code],
        "asked for again, it comes in front"
    );
    assert_eq!(model.grid.panes.len(), 4, "and no second one is made");
    Ok(())
}

/// A list pane opens a column 300 points wide on the far side from the chat, and a shelf
/// pane a row 150 points tall along the bottom; the next of each is a tab there.
#[test]
fn a_list_opens_a_side_column_and_a_terminal_a_bottom_shelf() -> Check {
    let mut model = window("a");
    open(&mut model, Files);
    open(&mut model, Terminal);
    let placed = drawn(&model)?;
    let (files, shelf) = (pane(&model, Files)?, pane(&model, Terminal)?);
    let (side, bottom) = (find(&placed, files)?, find(&placed, shelf)?);
    let chat = find(&placed, pane(&model, Chat)?)?;
    assert_eq!(side.w, DOCKED.w, "{placed:?}");
    assert_eq!(bottom.h, DOCKED.h, "{placed:?}");
    assert_eq!(
        bottom.y + bottom.h,
        FRAME_1.h,
        "the shelf is along the bottom"
    );
    // The record draws chat on the right, so the far side is the left edge.
    assert_eq!(side.x, 0.0, "the column is on the far side from the chat");
    assert!(chat.x > side.x);
    open(&mut model, Agents);
    open(&mut model, Trace);
    assert_eq!(drawn(&model)?.len(), 3, "no new slot for either");
    assert_eq!(tabs(&model, Agents)?, [Files, Agents]);
    assert_eq!(tabs(&model, Trace)?, [Terminal, Trace]);
    Ok(())
}

/// Where there is no room for a column or a shelf, the pane keeps the half it was cut.
#[test]
fn a_place_that_does_not_fit_keeps_the_half_it_was_cut() -> Check {
    let mut model = window("a");
    let (window, w, h) = (MAIN, MIN_PANE.w * 2.5, MIN_PANE.h * 2.5);
    let frame = crate::grid::windows::Frame {
        x: 0.0,
        y: 0.0,
        w,
        h,
    };
    change(&mut model, GridMsg::Moved { window, frame });
    open(&mut model, Files);
    let placed = model.grid.place(MAIN, Size { w, h });
    assert_eq!(placed.len(), 2);
    for (pane, rect) in &placed {
        assert!(
            rect.w >= MIN_PANE.w && rect.h >= MIN_PANE.h,
            "{pane:?} {rect:?}"
        );
    }
    Ok(())
}

/// Each session sees its own tabs of a shared stack, the tab in front follows the session,
/// and the slot rests where a session has none of them.
#[test]
fn a_stacks_tabs_follow_the_session_shown() -> Check {
    let mut model = window("a");
    open(&mut model, Changes);
    open(&mut model, Code);
    show(&mut model, "b");
    assert_eq!(kinds(&model)?, [Chat], "session b has opened neither");
    open(&mut model, Web);
    assert_eq!(kinds(&model)?, [Chat, Web]);
    assert_eq!(tabs(&model, Web)?, [Web], "b sees its own tab only");
    show(&mut model, "a");
    assert_eq!(tabs(&model, Changes)?, [Changes, Code]);
    assert_eq!(
        kinds(&model)?,
        [Chat, Code],
        "the tab a had in front is in front"
    );
    let web = pane(&model, Web)?;
    assert_eq!(model.grid.window_of(web), None, "b's tab is not drawn in a");
    let changes = pane(&model, Changes)?;
    change(&mut model, GridMsg::Front(changes));
    show(&mut model, "b");
    show(&mut model, "a");
    assert_eq!(
        kinds(&model)?,
        [Chat, Changes],
        "and the one it then chose, after"
    );
    Ok(())
}

/// The tab in front is one this session does not have: the first tab that it has takes the
/// slot, in the same place, with no session's memory of a front tab to lean on.
#[test]
fn a_front_tab_that_does_not_show_here_gives_the_slot_to_one_that_does() -> Check {
    let mut model = window("a");
    open(&mut model, Changes);
    let slot = find(&drawn(&model)?, pane(&model, Changes)?)?;
    show(&mut model, "b");
    open(&mut model, Changes);
    show(&mut model, "a");
    open(&mut model, Code);
    assert_eq!(kinds(&model)?, [Chat, Code], "a's own tab is in front");
    model.grid.fronts.clear();
    show(&mut model, "b");
    assert_eq!(kinds(&model)?, [Chat, Changes], "b has Changes, not Code");
    assert_eq!(find(&drawn(&model)?, pane(&model, Changes)?)?, slot);
    assert_eq!(tabs(&model, Changes)?, [Changes]);
    Ok(())
}

/// Closing the tab in front gives the slot to the next; closing the last tab closes the slot.
#[test]
fn closing_a_tab_gives_the_slot_to_the_next_and_the_last_closes_it() -> Check {
    let mut model = window("a");
    open(&mut model, Changes);
    open(&mut model, Code);
    let (changes, code) = (pane(&model, Changes)?, pane(&model, Code)?);
    change(&mut model, GridMsg::Close(code));
    assert_eq!(kinds(&model)?, [Chat, Changes]);
    assert_eq!(model.grid.kind(code), None, "the closed tab is forgotten");
    assert_eq!(tabs(&model, Changes)?, [Changes]);
    change(&mut model, GridMsg::Close(changes));
    assert_eq!(kinds(&model)?, [Chat]);
    Ok(())
}

/// A tab pulled out is its own window and leaves the slot to the next; docked back it is in
/// front again, in the place in the stack it had.
#[test]
fn a_tab_pulled_out_leaves_the_slot_and_docks_back_in_front() -> Check {
    let mut model = window("a");
    open(&mut model, Changes);
    open(&mut model, Code);
    open(&mut model, Web);
    let code = pane(&model, Code)?;
    change(&mut model, GridMsg::Front(code));
    let before = model.grid.clone();
    change(&mut model, GridMsg::Leave(code));
    assert_eq!(model.grid.windows.len(), 2);
    assert_eq!(
        tabs(&model, Web)?,
        [Changes, Web],
        "the stack goes on without it"
    );
    assert_eq!(kinds(&model)?, [Chat, Web], "the next tab took the slot");
    change(&mut model, GridMsg::Rejoin(code));
    assert_eq!(model.grid, before, "docked back, nothing has changed");
    Ok(())
}

/// The grid's file keeps its stacks, and one whose stack names two panes with slots, or
/// one pane alone, is refused.
#[test]
fn the_grid_file_keeps_its_stacks_and_refuses_a_broken_one() -> Check {
    let mut model = window("a");
    open(&mut model, Changes);
    open(&mut model, Code);
    let (text, read) = reread(&model)?;
    assert_eq!(read, model.grid);
    let (changes, code, chat) = (
        pane(&model, Changes)?,
        pane(&model, Code)?,
        pane(&model, Chat)?,
    );
    let good = format!("\"stacks\":[[{},{}]]", changes.0, code.0);
    assert!(text.contains(&good), "{text}");
    for bad in [
        format!("\"stacks\":[[{},{}]]", code.0, chat.0),
        format!("\"stacks\":[[{}]]", changes.0),
    ] {
        let read = serde_json::from_str::<Windows>(&text.replace(&good, &bad));
        assert!(read.is_err(), "{bad} was read");
    }
    Ok(())
}
