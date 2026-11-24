//! The lower half of the Panes panel: the panes this window shows in its session, under
//! the place each is in, then the kinds with something to show that it has no pane of.

use rune::{Cursor, El, Tx, el, row, text};
use yd_core::grid::listed::{Listed, Standing};
use yd_core::grid::step::GridMsg;
use yd_core::ids::WindowId;
use yd_core::msg::Msg;
use yd_core::pane::{PaneKind, Place};

use super::panel::{ITEM, KEY, WIDTH};
use super::{PANES, SHOW_SETTINGS, SHOW_THEMES};
use crate::chrome::tabs::{OUT, X, icon_of};
use crate::grid_view::drag::GLIDE;
use crate::grid_view::view::{self, Act, act, grip, show_pane};
use crate::kit::button::PRESS_SCALE;
use crate::kit::icons::glyph;
use crate::kit::label::section;
use crate::pane::PaneCx;
use crate::theme::look::{LABEL, META, colour};
use crate::theme::scale::{HOVER, RADIUS_CHIP};
use crate::win::{Hub, Win, dispatch};

/// Whether the grid shows panes of `kind`: Settings and Themes open in a window instead.
fn gridded(kind: PaneKind) -> bool {
    let apart = [SHOW_SETTINGS, SHOW_THEMES];
    let mut shown = PANES
        .iter()
        .filter(|(command, _)| !apart.contains(&command.id));
    shown.any(|(_, each)| *each == kind)
}

/// A press on a listed pane: it is drawn in this window and has the keyboard, and the
/// panel closes. One out in a window of its own is put back.
fn bring(win: &mut Win, tx: &mut Tx, entry: Listed) {
    tx.motion_curve(GLIDE);
    match entry.standing {
        Standing::Front => {}
        Standing::Behind => dispatch(win, Msg::Grid(GridMsg::Front(entry.pane))),
        Standing::Out => act(win, tx, Act::PopOut(entry.pane)),
    }
    grip(win, |grip| {
        (grip.focus, grip.picking) = (Some(entry.pane), false);
    });
}

/// The tab in front on the pane's own bar, when that says more than its name: the file a
/// viewer shows, a shell, a page.
fn showing(hub: &Hub, entry: &Listed, name: &str) -> Option<String> {
    let context = PaneCx {
        hub,
        pane: entry.pane,
        focused: false,
        edits: None,
    };
    let tabs = view::tabs_of(&context, entry.kind);
    let front = tabs.active().map(|tab| tab.title.clone());
    front.filter(|title| title != name)
}

fn pane_row(hub: &Hub, entry: Listed) -> El {
    let (tokens, grid) = (&hub.model.tokens, &hub.model.grid);
    let name = view::named(grid, entry.pane, entry.kind);
    let key = format!("{KEY}.pane-{}", entry.pane.0);
    let unseen = grid
        .panes
        .iter()
        .any(|pane| pane.id == entry.pane && pane.unseen);
    let (ink, stands, does) = match entry.standing {
        Standing::Front => (tokens.fg, "in front", format!("Go to {name}")),
        Standing::Behind => (tokens.dim, "a tab behind", format!("Bring {name} in front")),
        Standing::Out => (
            tokens.dim,
            "in a window of its own",
            format!("Put {name} back in this window"),
        ),
    };
    let new = if unseen { ", not seen yet" } else { "" };
    // The window's own chat cannot close: every other pane has the cross its bar has.
    let own_chat = entry.kind == PaneKind::Chat && grid.thing(entry.pane).is_none();
    let pane = entry.pane;
    let cross = (!own_chat).then(|| {
        row()
            .key(format!("{key}.close"))
            .label(format!("Close {name}"))
            .cursor(Cursor::Pointer)
            .press_scale(PRESS_SCALE)
            .child(glyph(&X, 8.0, tokens.faint))
            .on_press_tx(move |win: &mut Win, tx: &mut Tx| act(win, tx, Act::Close(pane)))
    });
    let note = showing(hub, &entry, &name).map(|title| {
        text(title)
            .font(META)
            .color(colour(tokens.faint))
            .truncate()
            .min_w(0.0)
            .shrink()
    });
    let dot = unseen.then(|| {
        el().key(format!("{key}.dot"))
            .size(5.0, 5.0)
            .radius(2.5)
            .fill(colour(tokens.accent))
    });
    let out = (entry.standing == Standing::Out).then(|| glyph(&OUT, 10.0, tokens.accent));
    row()
        .key(key)
        .w_full()
        .min_w(0.0)
        .h(ITEM)
        .px(5.0)
        .gap(6.0)
        .center_y()
        .radius(RADIUS_CHIP.of(tokens))
        .label(format!("{name}, {stands}{new}"))
        .help(does)
        .hover(colour(HOVER.of(tokens)))
        .cursor(Cursor::Pointer)
        .child(glyph(icon_of(entry.kind), 11.0, ink))
        .child(
            text(name)
                .font(LABEL)
                .color(colour(ink))
                .truncate()
                .min_w(0.0),
        )
        .children(dot)
        .children(out)
        .child(el().grow(1.0).min_w(0.0))
        .children(note)
        .children(cross)
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| bring(win, tx, entry))
}

/// A kind with something to show and no pane here: a press opens one in its place.
fn kind_item(hub: &Hub, kind: PaneKind) -> El {
    let tokens = &hub.model.tokens;
    row()
        .key(format!("{KEY}.kind.{}", kind.slug()))
        .w((WIDTH - 16.0) / 2.0)
        .h(ITEM)
        .px(5.0)
        .gap(6.0)
        .center_y()
        .radius(RADIUS_CHIP.of(tokens))
        .label(format!("{}, not open", kind.title()))
        .help(format!("Show {}", kind.title()))
        .hover(colour(HOVER.of(tokens)))
        .cursor(Cursor::Pointer)
        .child(glyph(icon_of(kind), 11.0, tokens.faint))
        .child(
            text(kind.title())
                .font(LABEL)
                .color(colour(tokens.faint))
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
            show_pane(win, tx, kind);
            grip(win, |grip| grip.picking = false);
        })
}

/// Each place that holds a pane, its panes under it, then the kinds not open.
pub(super) fn rows(hub: &Hub, window: WindowId) -> Vec<El> {
    let (tokens, grid) = (&hub.model.tokens, &hub.model.grid);
    let listed = grid.listed(window);
    let mut list = Vec::new();
    for place in Place::ALL {
        let here: Vec<Listed> = listed
            .iter()
            .copied()
            .filter(|entry| entry.place == place)
            .collect();
        if here.is_empty() {
            continue;
        }
        let count = here.len().to_string();
        let heading = section(tokens, place.slug(), Some(&count), None);
        list.push(heading.key(format!("{KEY}.place.{}", place.slug())));
        list.extend(here.into_iter().map(|entry| pane_row(hub, entry)));
    }
    let unopened = grid.unopened(window);
    let closed: Vec<PaneKind> = unopened.into_iter().filter(|kind| gridded(*kind)).collect();
    if !closed.is_empty() {
        list.push(section(tokens, "Not open", None, None).key(format!("{KEY}.closed")));
        let pairs = closed.chunks(2).map(|pair| {
            let items = pair.iter().map(|kind| kind_item(hub, *kind));
            row().gap(4.0).children(items)
        });
        list.extend(pairs);
    }
    list
}
