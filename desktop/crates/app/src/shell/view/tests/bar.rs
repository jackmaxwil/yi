//! The right-click rows of a pane's bar, in the real window: each is carried out on the pane
//! under the menu, whichever pane has the keyboard.

use yd_core::address::uri::Address;
use yd_core::command::by_kind::{
    PANE_TO_SHELF, PANE_TO_SIDE, PANE_TO_WORK, TAB_CLOSE_OTHERS, TAB_PULL_OUT,
};
use yd_core::grid::step::GridMsg;
use yd_core::ids::PaneId;
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::{PaneKind, Place};
use yd_ui::grid_view::aimed;
use yd_ui::win::dispatch;

use super::opens::menu;
use super::tabs::{front, stacked};
use super::*;

const SPLIT_RIGHT: &str = "window.context-menu.row.tab.split-right";
const SPLIT_DOWN: &str = "window.context-menu.row.tab.split-down";
const PULL_OUT: &str = "window.context-menu.row.tab.pull-out";
const KEEP: &str = "window.context-menu.row.tab.keep";
const ACROSS: &str = "window.context-menu.row.tab.keep-across";
const OTHERS: &str = "window.context-menu.row.tab.close-others";
const WORK: &str = "window.context-menu.row.pane.to-work";
const SIDE: &str = "window.context-menu.row.pane.to-side";
const SHELF: &str = "window.context-menu.row.pane.to-shelf";

/// The panes of `kind` the grid holds, in the order it made them.
pub(super) fn of(runtime: &Runtime<Win>, kind: PaneKind) -> Result<Vec<PaneId>, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    let panes = hub.model.grid.panes.iter();
    Ok(panes.filter(|p| p.kind == kind).map(|p| p.id).collect())
}

pub(super) fn one(runtime: &Runtime<Win>, kind: PaneKind) -> Result<PaneId, Box<dyn Error>> {
    match of(runtime, kind)?[..] {
        [pane] => Ok(pane),
        ref found => Err(format!("{kind:?} panes: {found:?}").into()),
    }
}

/// Said on a tab behind the one in front, closing the others leaves that tab alone in the
/// slot. A pane alone is not offered the row.
#[test]
fn close_others_leaves_the_tab_under_the_menu_alone_in_its_slot() -> Check {
    let mut runtime = stacked()?;
    assert_eq!(front(&runtime)?, "ipython");
    let debugger = one(&runtime, PaneKind::Debugger)?;
    menu(&mut runtime, "", &format!(".tab-pane-{}.", debugger.0))?;
    assert!(live(&runtime, OTHERS), "a tab among others is offered it");
    tap(&mut runtime, OTHERS)?;
    assert_eq!(front(&runtime)?, "debugger", "it has the slot");
    for gone in [PaneKind::Changes, PaneKind::IPython] {
        assert_eq!(of(&runtime, gone)?, [], "{gone:?} closed");
    }
    menu(&mut runtime, "debugger-", "")?;
    assert!(live(&runtime, SPLIT_RIGHT), "the menu is the bar's");
    assert!(!live(&runtime, OTHERS), "alone, there are no others");
    Ok(())
}

/// Keep makes a pane the layout seated the person's and ticks; kept across sessions the tick
/// moves, and picked again it is back to this session. The chat is offered neither.
#[test]
fn keeping_a_tab_ticks_its_row_and_across_sessions_moves_the_tick() -> Check {
    let mut runtime = settled("shell/everything")?;
    let changes = one(&runtime, PaneKind::Changes)?;
    let kept = |runtime: &Runtime<Win>| -> Result<Option<bool>, Box<dyn Error>> {
        Ok(runtime.state.hub.try_borrow()?.model.grid.kept(changes))
    };
    let ticks = |runtime: &mut Runtime<Win>| -> Result<(bool, bool), Box<dyn Error>> {
        menu(runtime, "changes-", "")?;
        let shown = runtime.elements();
        let tick = |key| row_text(&shown, key).map(|row| row.contains('✓'));
        Ok((tick(KEEP)?, tick(ACROSS)?))
    };
    // The scene poses its panes as the person's; a layout or a rule seats the ones it opens.
    {
        let mut hub = runtime.state.hub.try_borrow_mut()?;
        let mut panes = hub.model.grid.panes.iter_mut();
        let found = panes.find(|pane| pane.id == changes);
        found.ok_or("no Changes pane")?.seated = true;
    }
    assert_eq!(kept(&runtime)?, None, "seated by the layout");
    assert_eq!(ticks(&mut runtime)?, (false, false));
    tap(&mut runtime, KEEP)?;
    assert_eq!(kept(&runtime)?, Some(false), "kept in this session");
    assert_eq!(ticks(&mut runtime)?, (true, false));
    tap(&mut runtime, ACROSS)?;
    assert_eq!(kept(&runtime)?, Some(true), "kept across sessions");
    assert_eq!(ticks(&mut runtime)?, (false, true));
    tap(&mut runtime, ACROSS)?;
    assert_eq!(kept(&runtime)?, Some(false), "picked again: this session");
    menu(&mut runtime, "chat-", "")?;
    assert!(live(&runtime, SPLIT_RIGHT), "the chat's bar has a menu");
    assert!(
        !live(&runtime, KEEP) && !live(&runtime, ACROSS),
        "no keeping"
    );
    Ok(())
}

/// Split down draws a second pane of the kind right under the one the menu is on; pull out
/// puts that one in a window of its own, where the row is offered no more.
#[test]
fn split_down_and_pull_out_act_on_the_pane_under_the_menu() -> Check {
    let mut runtime = settled("shell/everything")?;
    let files = one(&runtime, PaneKind::Files)?;
    menu(&mut runtime, "files-", "")?;
    tap(&mut runtime, SPLIT_DOWN)?;
    let both = of(&runtime, PaneKind::Files)?;
    let [_, copy] = both[..] else {
        return Err(format!("Files panes after the split: {both:?}").into());
    };
    let frame = |runtime: &Runtime<Win>, pane: PaneId| {
        let key = format!("files-{}.pane-frame", pane.0);
        let found = by_key(&runtime.elements(), &key).map(|frame| frame.rect);
        found.ok_or(format!("no {key}"))
    };
    let ([x, y, _, h], [below_x, below_y, ..]) = (frame(&runtime, files)?, frame(&runtime, copy)?);
    assert!((below_x - x).abs() < 0.5, "under it: {below_x} is not {x}");
    assert!((below_y - (y + h)).abs() < 2.0, "right under it: {below_y}");
    let offered = |runtime: &Runtime<Win>| -> Result<bool, Box<dyn Error>> {
        let hub = runtime.state.hub.try_borrow()?;
        let address = Address::new(Kind::Pane, files.0.to_string())?;
        Ok(aimed::offered(&hub, TAB_PULL_OUT, &address))
    };
    assert!(offered(&runtime)?, "a docked pane can be pulled out");
    menu(&mut runtime, "files-", ".tab-0.")?;
    tap(&mut runtime, PULL_OUT)?;
    let out = {
        let hub = runtime.state.hub.try_borrow()?;
        hub.model.grid.origin_of(files)
    };
    assert_eq!(out, Some(root::MAIN), "it left the main window");
    assert!(!offered(&runtime)?, "out, it has no window to leave");
    Ok(())
}

/// On one of a pane's own tabs, closing the others closes its other tabs and leaves the
/// pane; said of the pane whole, its own tabs are no others.
#[test]
fn close_others_on_a_panes_own_tab_closes_its_other_tabs() -> Check {
    let mut runtime = settled("shell/everything")?;
    let code = one(&runtime, PaneKind::Code)?;
    let tab = |n: u32| format!("code-{}.tab-{n}.pane-tab", code.0);
    assert!(
        live(&runtime, &tab(1)) && live(&runtime, &tab(2)),
        "two files"
    );
    let offered = |runtime: &Runtime<Win>, kind: Kind, id: String| {
        let hub = runtime.state.hub.try_borrow()?;
        let address = Address::new(kind, id)?;
        Ok::<bool, Box<dyn Error>>(aimed::offered(&hub, TAB_CLOSE_OTHERS, &address))
    };
    assert!(offered(&runtime, Kind::Tab, format!("{}/2", code.0))?);
    let whole = offered(&runtime, Kind::Pane, code.0.to_string())?;
    assert!(!whole, "the pane alone in its slot has no others");
    menu(&mut runtime, &format!("code-{}.tab-2.", code.0), "")?;
    tap(&mut runtime, OTHERS)?;
    assert!(live(&runtime, &tab(2)), "the tab under the menu stays");
    assert!(!live(&runtime, &tab(1)), "the other file closed");
    assert_eq!(one(&runtime, PaneKind::Code)?, code, "the pane stays");
    menu(&mut runtime, &format!("code-{}.tab-2.", code.0), "")?;
    assert!(!live(&runtime, OTHERS), "one tab left: no others");
    Ok(())
}

/// The menu of a pane low in the window opens with every row inside the window, and in a
/// window shorter than the menu it is no taller than the window.
#[test]
fn the_bars_menu_stays_inside_the_window() -> Check {
    let mut runtime = settled("shell/everything")?;
    for tall in [750.0, 480.0] {
        runtime.set_viewport((1200.0, tall), 1.0);
        rest(&mut runtime);
        menu(&mut runtime, "terminal-", "")?;
        let shown = runtime.elements();
        let [_, y, _, h] = by_key(&shown, "window.context-menu").ok_or("no menu")?.rect;
        assert!(live(&runtime, KEEP), "the bar's rows are in it");
        assert!(y >= 0.0 && y + h <= tall + 0.5, "{y} + {h} in {tall}");
        strike(&mut runtime, Key::Escape, Mods::default());
        rest(&mut runtime);
    }
    Ok(())
}

/// A pane's menu offers the places it is not in. Moved, it is drawn in that place's stack,
/// and the menu then offers the place it left. The chat is offered no move.
#[test]
fn move_to_a_place_puts_the_pane_there_and_offers_the_place_it_left() -> Check {
    let mut runtime = settled("shell/everything")?;
    let files = one(&runtime, PaneKind::Files)?;
    let place = |runtime: &Runtime<Win>| -> Result<Option<Place>, Box<dyn Error>> {
        Ok(runtime.state.hub.try_borrow()?.model.grid.slot_place(files))
    };
    let rows = |runtime: &Runtime<Win>| [WORK, SIDE, SHELF].map(|row| live(runtime, row));
    assert_eq!(place(&runtime)?, Some(Place::Side));
    menu(&mut runtime, "files-", "")?;
    assert_eq!(rows(&runtime), [true, false, true], "not where it is");
    tap(&mut runtime, WORK)?;
    assert_eq!(place(&runtime)?, Some(Place::Work));
    let frame = format!("files-{}.pane-frame", files.0);
    assert!(live(&runtime, &frame), "drawn, in front of the work stack");
    let tabs = {
        let hub = runtime.state.hub.try_borrow()?;
        hub.model.grid.tabs(root::MAIN, files).len()
    };
    assert_eq!(tabs, 2, "a tab of that stack");
    menu(&mut runtime, "files-", ".tab-0.")?;
    assert_eq!(rows(&runtime), [false, true, true], "it can go back");
    tap(&mut runtime, SHELF)?;
    assert_eq!(place(&runtime)?, Some(Place::Shelf));
    menu(&mut runtime, "files-", ".tab-0.")?;
    tap(&mut runtime, SIDE)?;
    assert_eq!(
        place(&runtime)?,
        Some(Place::Side),
        "each row its own place"
    );
    menu(&mut runtime, "chat-", "")?;
    assert_eq!(rows(&runtime), [false; 3], "the chat stays in its place");
    strike(&mut runtime, Key::Escape, Mods::default());
    runtime.update(move |win, _| dispatch(win, Msg::Grid(GridMsg::Leave(files))));
    rest(&mut runtime);
    let hub = runtime.state.hub.try_borrow()?;
    let address = Address::new(Kind::Pane, files.0.to_string())?;
    let out = [PANE_TO_WORK, PANE_TO_SIDE, PANE_TO_SHELF];
    let offered = out.map(|row| aimed::offered(&hub, row, &address));
    assert_eq!(
        offered, [false; 3],
        "out in its own window, it has no place"
    );
    Ok(())
}
