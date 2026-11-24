//! A file shown beside: a code pane about that one file, in a slot or a window of its own.

use rune::Tx;
use yd_core::address::uri::Address;
use yd_core::command::run::shown_path;
use yd_core::grid::layout::Edge;
use yd_core::grid::step::GridMsg;
use yd_core::ids::PaneId;
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;

use crate::grid_view::drag::GLIDE;
use crate::grid_view::view::{Act, act, plain};
use crate::win::{Hub, Win, dispatch};

/// The pane of this window that is about `file`, drawn or out in a window of its own.
fn about(hub: &Hub, file: &Address) -> Option<PaneId> {
    let is = |pane: &&yd_core::grid::windows::Pane| {
        pane.kind == PaneKind::Code && pane.thing.as_ref() == Some(file)
    };
    hub.model.grid.panes.iter().find(is).map(|pane| pane.id)
}

/// Shows the code pane about `file`, with the keys. A new one is cut from `at`, else from
/// the right of this window's viewer or its place's tabs; `apart`, it leaves for a window.
pub fn show(win: &mut Win, tx: &mut Tx, file: &Address, at: Option<(PaneId, Edge)>, apart: bool) {
    let Ok(thing) = Address::new(Kind::File, file.id()) else {
        return;
    };
    let (window, kind) = (win.window, PaneKind::Code);
    let (pane, edge) = match at {
        Some((pane, edge)) => (Some(pane), edge),
        None => (plain(win, kind), Edge::Right),
    };
    tx.motion_curve(GLIDE);
    // Bound for a window of its own it cuts no slot here: it is a tab until it leaves.
    let shown = match apart {
        true => GridMsg::About {
            window,
            kind,
            thing: thing.clone(),
        },
        false => GridMsg::Beside {
            window,
            kind,
            thing: thing.clone(),
            pane,
            edge,
        },
    };
    dispatch(win, Msg::Grid(shown));
    let made = win.hub.try_borrow().ok().and_then(|hub| {
        let pane = about(&hub, &thing)?;
        Some((pane, hub.model.grid.origin_of(pane).is_some()))
    });
    match made {
        Some((pane, false)) if apart => dispatch(win, Msg::Grid(GridMsg::Leave(pane))),
        Some((pane, false)) => act(win, tx, Act::Focus(pane)),
        // Out in a window of its own already, or refused: there is nothing here to focus.
        Some((_, true)) | None => {}
    }
}

/// Splits `pane` at `edge`. A code pane with a file to show gets that file beside it, in a
/// pane about it: a second viewer would show whatever the first does. Any other is copied.
pub fn split(win: &mut Win, tx: &mut Tx, pane: PaneId, edge: Edge) {
    let file = win.hub.try_borrow().ok().and_then(|hub| {
        let code = hub.model.grid.kind(pane) == Some(PaneKind::Code);
        let path = shown_path(&hub.model, pane).filter(|_| code)?;
        Address::new(Kind::File, path).ok()
    });
    match file {
        Some(file) => show(win, tx, &file, Some((pane, edge)), false),
        None => {
            tx.motion_curve(GLIDE);
            dispatch(win, Msg::Grid(GridMsg::SplitAt { pane, edge }));
        }
    }
}
