//! The right-click rows of a pane's bar, carried out on the pane the menu is about.

use rune::Tx;
use yd_core::address::uri::Address;
use yd_core::command::by_kind::{
    self, PANE_ALWAYS, PANE_NEVER, TAB_CLOSE_OTHERS, TAB_KEEP, TAB_KEEP_ACROSS, TAB_PULL_OUT,
    TAB_SPLIT_DOWN, TAB_SPLIT_RIGHT, pane_named,
};
use yd_core::command::table::Target;
use yd_core::grid::layout::Edge;
use yd_core::grid::step::{GridMsg, takes};
use yd_core::ids::{CommandId, PaneId, TabId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;

use crate::chrome::tabs;
use crate::grid_view::drag::GLIDE;
use crate::grid_view::view::{Act, act, hears_tabs, kind_of, split, tabs_of};
use crate::pane::PaneCx;
use crate::win::{Hub, Win, dispatch};

/// The pane's own tabs that closing the others closes: each with a cross but the one
/// `address` names. None when the address names the pane whole: its own tabs all stay.
fn own_others(hub: &Hub, pane: PaneId, address: &Address) -> Vec<TabId> {
    let kept = address.id().split('/').nth(1);
    let kept = kept.and_then(|tab| tab.parse().ok()).map(TabId);
    let (Kind::Tab, Some(kept), Some(kind)) = (address.kind(), kept, kind_of(hub, pane)) else {
        return Vec::new();
    };
    let context = PaneCx {
        hub,
        pane,
        focused: false,
        edits: None,
    };
    let own = hears_tabs(hub, kind);
    let shown = tabs_of(&context, kind);
    let others = shown.tabs.iter().filter(|tab| tab.id != kept);
    let closing = others.filter(|tab| tabs::closes(own, tab));
    closing.map(|tab| tab.id).collect()
}

/// Whether the menu offers `command` for what `address` names. A row that is not a bar's,
/// or a thing that is no pane, is always offered.
pub fn offered(hub: &Hub, command: CommandId, address: &Address) -> bool {
    let grid = &hub.model.grid;
    let Some(pane) = pane_named(address) else {
        return true;
    };
    let chat = grid.kind(pane) == Some(PaneKind::Chat) && grid.thing(pane).is_none();
    match command {
        PANE_ALWAYS | PANE_NEVER => by_kind::word(grid, command, address).is_some(),
        // The window's own chat shows every session: there is nothing to keep it from.
        TAB_KEEP | TAB_KEEP_ACROSS => !chat,
        // A pane out in its own window has no window to be pulled out of.
        TAB_PULL_OUT => grid.origin_of(pane).is_none(),
        TAB_CLOSE_OTHERS => {
            !grid.mates(pane).is_empty() || !own_others(hub, pane, address).is_empty()
        }
        // A move is offered where the grid would make it: not to the place the pane is in,
        // nor to a pane out in its own window or the last one of its window.
        _ => by_kind::moves(command)
            .is_none_or(|place| !chat && takes(grid, GridMsg::Place { pane, place })),
    }
}

/// Carries out a bar's row on the pane `target` names. False for any other command, or one
/// the menu does not offer for that pane.
pub fn carry(win: &mut Win, tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    let rows = [
        TAB_SPLIT_RIGHT,
        TAB_SPLIT_DOWN,
        TAB_PULL_OUT,
        TAB_KEEP,
        TAB_KEEP_ACROSS,
        TAB_CLOSE_OTHERS,
    ];
    let Target::Thing(address) = target else {
        return false;
    };
    let place = by_kind::moves(command);
    let ours = rows.contains(&command) || place.is_some();
    let Some(pane) = pane_named(address).filter(|_| ours) else {
        return false;
    };
    let read = win.hub.try_borrow().ok().map(|hub| {
        let grid = &hub.model.grid;
        let closing = (grid.mates(pane), own_others(&hub, pane, address));
        (offered(&hub, command, address), grid.kept(pane), closing)
    });
    let Some((true, kept, (mates, own))) = read else {
        return false;
    };
    tx.motion_curve(GLIDE);
    if let Some(place) = place {
        dispatch(win, Msg::Grid(GridMsg::Place { pane, place }));
        return true;
    }
    match command {
        TAB_SPLIT_RIGHT => split(win, pane, Edge::Right),
        TAB_SPLIT_DOWN => split(win, pane, Edge::Bottom),
        TAB_CLOSE_OTHERS => {
            for other in mates {
                act(win, tx, Act::Close(other));
            }
            for tab in own {
                act(win, tx, Act::CloseOne(pane, tab));
            }
        }
        _ => {
            // In front first: the grid pulls out and scopes the pane drawn, and a pane the
            // person brought in front is theirs, which is all that keeping it is.
            dispatch(win, Msg::Grid(GridMsg::Front(pane)));
            match command {
                TAB_PULL_OUT => act(win, tx, Act::PopOut(pane)),
                TAB_KEEP_ACROSS => act(win, tx, Act::Across(pane, kept != Some(true))),
                _ => {}
            }
        }
    }
    true
}
