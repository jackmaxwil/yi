//! A thing in the hand: a file dragged from where it is listed and let go on a pane, on a
//! pane's edge or on the window's. The pane about it lands where a carried pane would.

use std::collections::BTreeMap;

use rune::{Drag, El, Phase, Shadow, Tx, row, text};
use yd_core::address::uri::Address;
use yd_core::grid::layout::{Point, Size};
use yd_core::grid::step::GridMsg;
use yd_core::grid::swap::Zone;
use yd_core::ids::{PaneId, WindowId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;

use crate::chrome::tabs::icon_of;
use crate::grid_view::drag::{At, GLIDE, THRESHOLD};
use crate::grid_view::view::{Act, act};
use crate::kit::icons::glyph;
use crate::theme::look::{LABEL, colour};
use crate::theme::scale::{POPOVER, RADIUS_CHIP, RING_MENU};
use crate::win::{Hub, Win, dispatch};

pub const CHIP: &str = "grid.carried";
/// Over the drop preview and every pane.
const OVER_ALL: i16 = 3;
/// The chip hangs this far right of and under the pointer, clear of its tip.
const HANG: [f32; 2] = [10.0, 12.0];

/// What the pointer carries over this window's grid.
#[derive(Debug, Clone, PartialEq)]
pub struct Carried {
    pub thing: Address,
    /// The pane it was listed in: no place to drop it.
    pub from: PaneId,
    /// The pointer, in the grid's points.
    pub at: Point,
}

/// The thing in each window's hand. It lives in the hub so that a scene can pose one.
#[derive(Debug, Default)]
pub struct Hands(pub BTreeMap<WindowId, Carried>);

/// The kind of pane that is about `thing`: a file's is a code pane. None for a thing that
/// has no pane of its own, which is not carried.
fn pane_about(thing: &Address) -> Option<PaneKind> {
    (thing.kind() == Kind::File).then_some(PaneKind::Code)
}

fn held(hub: &Hub, window: WindowId) -> Option<&Carried> {
    hub.panes.get::<Hands>(None)?.0.get(&window)
}

/// Puts `carried` in this window's hand, or empties it.
fn hold(win: &mut Win, carried: Option<Carried>) {
    let window = win.window;
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    let Some(hands) = hub.panes.get_mut::<Hands>(None) else {
        return;
    };
    match carried {
        Some(carried) => drop(hands.0.insert(window, carried)),
        None => drop(hands.0.remove(&window)),
    }
}

/// Where the thing in hand would land in a grid of `size`; none with nothing in hand, off
/// the grid, or over the pane it came from.
pub fn zone(win: &Win, hub: &Hub, size: Size) -> Option<Zone> {
    let carried = held(hub, win.window)?;
    let about = (pane_about(&carried.thing)?, &carried.thing);
    let grid = &hub.model.grid;
    grid.zone_for(win.window, size, about, carried.from, carried.at)
}

/// One step of a drag that began on an element of pane `from` standing for `thing`. Carried
/// only once the pointer has gone the pane drag's own distance, so a click stays a click.
pub fn hand(win: &mut Win, thing: &Address, from: PaneId, at: At, drag: &Drag, tx: &mut Tx) {
    let Some(kind) = pane_about(thing) else {
        return;
    };
    match drag.phase {
        Phase::Down => {}
        Phase::Move => {
            let far = drag.dx.hypot(drag.dy) >= THRESHOLD;
            let window = win.window;
            let carrying = win.hub.try_borrow();
            let carrying = carrying.is_ok_and(|hub| held(&hub, window).is_some());
            if !far && !carrying {
                return;
            }
            let carried = Carried {
                thing: thing.clone(),
                from,
                at: at.point(drag),
            };
            hold(win, Some(carried));
        }
        Phase::Up => {
            let landing = win.hub.try_borrow().ok().and_then(|hub| {
                let zone = zone(win, &hub, at.size)?;
                zone.lands.map(|_| zone.spot)
            });
            hold(win, None);
            let Some(spot) = landing.filter(|_| !drag.cancelled()) else {
                return;
            };
            tx.motion_curve(GLIDE);
            let (window, size) = (win.window, at.size);
            let put = GridMsg::Put {
                window,
                kind,
                thing: thing.clone(),
                at: spot,
                size,
            };
            dispatch(win, Msg::Grid(put));
            let made = win.hub.try_borrow().ok().and_then(|hub| {
                hub.model.grid.drawn_about(window, kind, thing)
            });
            if let Some(made) = made {
                act(win, tx, Act::Focus(made));
            }
        }
    }
}

/// The thing in hand, drawn by the pointer: its pane's icon and its name.
pub fn chip(win: &Win, hub: &Hub) -> Option<El> {
    let carried = held(hub, win.window)?;
    let (tokens, kind) = (&hub.model.tokens, pane_about(&carried.thing)?);
    let id = carried.thing.id();
    let name = id.rsplit('/').next().unwrap_or(id);
    let chip = row()
        .key(CHIP)
        .absolute(carried.at.x + HANG[0], carried.at.y + HANG[1])
        .h(20.0)
        .px(6.0)
        .gap(5.0)
        .center_y()
        .radius(RADIUS_CHIP.of(tokens))
        .fill(colour(POPOVER.of(tokens)))
        .box_shadow(Shadow {
            spread: 1.0,
            color: colour(RING_MENU.of(tokens)),
            ..Shadow::default()
        })
        .label(format!("Carrying {name}"))
        .paint_order(OVER_ALL)
        .inert()
        .child(glyph(icon_of(kind), 11.0, tokens.dim))
        .child(text(name).font(LABEL).color(colour(tokens.fg)));
    Some(chip)
}
