//! The Panes panel under the top bar's button: a picture of every layout, the chat side, and
//! every pane kind with a switch. The layouts kept are named, renamed and deleted in place.

use rune::{Align, Cursor, Cx, El, Key, KeyEvent, Side, TextEdit, Tx, col, el, row, text};
use rune::{spacer, text_input};
use yd_core::command::keys::Map;
use yd_core::command::table::{FLIP_CHAT, Route, Run};
use yd_core::grid::layout::{ChatSide, Size};
use yd_core::grid::saved::{
    FIRST_OWN, LAST_KEYED, MAX_OWN, OwnId, Shape, find as find_saved, named,
};
use yd_core::grid::step::GridMsg;
use yd_core::ids::{PaneId, WindowId};
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::tokens::Tokens;

use super::{BUTTON, LAYOUTS, PANES, SHOW_SETTINGS, SHOW_THEMES, own_command};
use crate::chrome::tabs::{PLUS, icon_of};
use crate::grid_view::view::{self, Act, Grips, act, grip, show_pane};
use crate::kit::button::{Button, button};
use crate::kit::icons::glyph;
use crate::kit::label::section;
use crate::kit::segmented::segmented;
use crate::kit::toggle::toggle;
use crate::menu::command_box;
use crate::menu::context::popover_shadow;
use crate::theme::look::{LABEL, NONE, colour};
use crate::theme::scale::{
    HOVER, LINE_MENU, LINE_PICKED, POPOVER, RADIUS_BADGE, RADIUS_CHIP, RADIUS_MENU,
};
use crate::win::{Field, Hub, Win, dispatch};

/// What the panel is asking of a layout the person keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Naming {
    #[default]
    Idle,
    /// A name for the arrangement the window shows now.
    New,
    Rename(OwnId),
    /// Rename or Delete, after a right-click on its picture.
    Offered(OwnId),
}

pub const KEY: &str = "grid.panel";
const NAME: Field = (None, "layout-name");
const NAME_KEY: &str = "grid.panel.name";
const WIDTH: f32 = 300.0;
const PICTURE: f32 = 40.0;
const ITEM: f32 = 20.0;
/// Over the panes, whose frames and lines paint above the window's other layers.
const OVER_ALL: i16 = 8;

fn grip_of(hub: &Hub, window: WindowId) -> view::Grip {
    let grips = hub.panes.get::<Grips>(None);
    grips.map(|grips| grips.of(window)).unwrap_or_default()
}

fn naming(win: &mut Win, now: Naming) {
    grip(win, |grip| grip.naming = now);
}

/// A dismissal while a name is asked cancels the name only; the next one closes the panel.
pub fn close(win: &mut Win) {
    let asked = win
        .hub
        .try_borrow()
        .map(|hub| grip_of(&hub, win.window).naming);
    let leaves = asked.is_ok_and(|asked| asked == Naming::Idle);
    grip(win, |grip| {
        grip.naming = Naming::Idle;
        grip.picking &= !leaves;
    });
    win.edits.remove(&NAME);
}

/// Asks for a name in the panel, the field seeded with `seed`.
fn ask_name(win: &mut Win, tx: &mut Tx, now: Naming, seed: &str) {
    win.edits.insert(NAME, TextEdit::new(seed));
    naming(win, now);
    tx.focus_when_shown(NAME_KEY, false);
}

/// Return keeps the name typed (refused, the field stays), Esc leaves the field.
fn typed(win: &mut Win, event: &KeyEvent, _tx: &mut Tx) -> bool {
    let window = win.window;
    let asked = win.hub.try_borrow().map(|hub| grip_of(&hub, window).naming);
    let Ok(asked) = asked else { return false };
    match event.key {
        Key::Escape => naming(win, Naming::Idle),
        Key::Enter => {
            let name = win.edits.get(&NAME).map(|edit| edit.text.clone());
            let Some(name) = name.filter(|name| named(name).is_some()) else {
                return true;
            };
            let change = match asked {
                Naming::New => GridMsg::Keep { window, name },
                Naming::Rename(own) => GridMsg::Rename { own, name },
                Naming::Idle | Naming::Offered(_) => return true,
            };
            dispatch(win, Msg::Grid(change));
            naming(win, Naming::Idle);
            win.edits.remove(&NAME);
        }
        _ => return false,
    }
    true
}

/// The next number a kept layout gets, none when eight are kept.
fn next_number(hub: &Hub) -> Option<u8> {
    let own = &hub.model.grid.own;
    let numbers = FIRST_OWN..FIRST_OWN.saturating_add(MAX_OWN);
    numbers
        .into_iter()
        .find(|n| own.iter().all(|kept| kept.id.0 != *n))
}

struct Picture<'a> {
    key: String,
    shape: Shape,
    current: bool,
    /// The panel opens with the keyboard here: the layout shown, else the first.
    first: bool,
    help: String,
    room: Size,
    side: ChatSide,
    tokens: &'a Tokens,
}

/// One layout as it would be drawn here: every pane where `place` puts it, chat tinted.
fn picture(drawn: Picture<'_>) -> El {
    let tokens = drawn.tokens;
    let height = drawn.room.h * PICTURE / drawn.room.w.max(1.0);
    let cells = drawn.shape.picture(drawn.room, drawn.side, PICTURE);
    let key = drawn.key;
    let cells = cells.into_iter().enumerate().map(|(n, (kind, at))| {
        let fill = match kind {
            PaneKind::Chat => tokens.accent.alpha(0.45),
            _ => tokens.raised,
        };
        el().key(format!("{key}.{}-{n}", kind.slug()))
            .absolute(at.x + 0.5, at.y + 0.5)
            .size((at.w - 1.0).max(0.0), (at.h - 1.0).max(0.0))
            .radius(1.5)
            .fill(colour(fill))
    });
    let cells: Vec<El> = cells.collect();
    let ring = if drawn.current {
        LINE_PICKED.of(tokens)
    } else {
        NONE
    };
    let said = match drawn.current {
        true => format!("{}, shown", drawn.help),
        false => drawn.help.clone(),
    };
    let tile = col()
        .key(key)
        .padding(2.0)
        .radius(RADIUS_BADGE.of(tokens))
        .border_sides([1.0; 4], colour(ring))
        .hover(colour(HOVER.of(tokens)))
        .cursor(Cursor::Pointer)
        .label(said)
        .help(drawn.help)
        .child(el().size(PICTURE, height).children(cells));
    match drawn.first {
        true => tile.autofocus(),
        false => tile,
    }
}

/// The saved five, then the layouts kept, then the button that keeps one more.
fn layouts(hub: &Hub, window: WindowId, room: Size, keys: &Map) -> El {
    let (tokens, grid) = (&hub.model.tokens, &hub.model.grid);
    let shown = grid.window(window);
    let (saved, own) = shown.map_or((None, None), |shown| (shown.saved, shown.own));
    let named_key = |id| keys.hint(id).unwrap_or_else(|| "no key".to_owned());
    let marked = own.is_some() || LAYOUTS.iter().any(|(_, slug)| saved == Some(*slug));
    let mut row_of: Vec<El> = Vec::new();
    for (at, (command, slug)) in LAYOUTS.iter().enumerate() {
        let Some(layout) = find_saved(slug) else {
            continue;
        };
        let pressed = *slug;
        let drawn = picture(Picture {
            key: format!("{KEY}.layout.{slug}"),
            shape: layout.shape(),
            current: saved == Some(*slug),
            first: saved == Some(*slug) || (!marked && at == 0),
            help: format!("{} {}", layout.name, named_key(command.id)),
            room,
            side: grid.side,
            tokens,
        });
        row_of.push(drawn.on_press_tx(move |win: &mut Win, tx: &mut Tx| {
            act(win, tx, Act::Show(pressed));
        }));
    }
    for kept in &grid.own {
        let id = kept.id;
        let key = own_command(id).map(|command| named_key(command.id));
        let drawn = picture(Picture {
            key: format!("{KEY}.layout.own-{}", id.0),
            shape: kept.shape.clone(),
            current: own == Some(id),
            first: own == Some(id),
            help: format!("{} {}", kept.name, key.unwrap_or_default()),
            room,
            side: grid.side,
            tokens,
        });
        let drawn = drawn
            .on_press_tx(move |win: &mut Win, tx: &mut Tx| act(win, tx, Act::Wear(id)))
            .on_secondary_press(move |win: &mut Win, _, _: &mut Tx| {
                naming(win, Naming::Offered(id));
            });
        row_of.push(drawn);
    }
    let height = room.h * PICTURE / room.w.max(1.0);
    let full = next_number(hub).is_none();
    let help = match full {
        true => format!("{MAX_OWN} layouts of your own is the most; delete one first"),
        false => "Save this arrangement as a layout".to_owned(),
    };
    let ink = if full { tokens.faint } else { tokens.dim };
    let save = col()
        .key(format!("{KEY}.save"))
        .w(PICTURE + 4.0)
        .h(height + 4.0)
        .center()
        .radius(RADIUS_BADGE.of(tokens))
        .border_sides([1.0; 4], colour(LINE_MENU.of(tokens)))
        .label(help.clone())
        .help(help)
        .child(glyph(&PLUS, 12.0, ink));
    let save = match full {
        true => save,
        false => save
            .hover(colour(HOVER.of(tokens)))
            .cursor(Cursor::Pointer)
            .on_press_tx(|win: &mut Win, tx: &mut Tx| ask_name(win, tx, Naming::New, "")),
    };
    row_of.push(save);
    row()
        .w_full()
        .min_w(0.0)
        .flex_wrap()
        .gap(4.0)
        .children(row_of)
}

/// The line under the pictures while a name is asked for or a kept layout is offered.
fn asking(win: &Win, hub: &Hub, now: Naming) -> Option<El> {
    let tokens = &hub.model.tokens;
    let kept = |id: OwnId| hub.model.grid.own.iter().find(|own| own.id == id);
    match now {
        Naming::Idle => None,
        Naming::New | Naming::Rename(_) => {
            let keyless = next_number(hub).is_some_and(|n| n > LAST_KEYED);
            let hole = match (now, keyless) {
                (Naming::New, true) => "Name it: ⌥⌘6 to ⌥⌘9 are taken, so it gets no key",
                (Naming::New, false) => "Name this layout, then Return",
                _ => "A new name, then Return",
            };
            let seed = TextEdit::default();
            let edit = win.edits.get(&NAME).unwrap_or(&seed);
            let field = text_input(edit, LABEL)
                .key(NAME_KEY)
                .single_line()
                .h(ITEM)
                .px(6.0)
                .w_full()
                .min_w(0.0)
                .radius(RADIUS_CHIP.of(tokens))
                .fill(colour(tokens.raised))
                .color(colour(tokens.fg))
                .placeholder(hole, colour(tokens.faint))
                .label(hole)
                .on_edit(|win: &mut Win| win.edits.entry(NAME).or_default())
                .on_input_key(typed);
            Some(field)
        }
        Naming::Offered(id) => {
            let name = kept(id).map(|own| own.name.clone()).unwrap_or_default();
            let rename = button(tokens, Button::Ghost, "Rename", None)
                .key(format!("{KEY}.rename"))
                .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
                    let seed = win.hub.try_borrow().ok().and_then(|hub| {
                        let kept = hub.model.grid.own.iter().find(|own| own.id == id);
                        kept.map(|own| own.name.clone())
                    });
                    ask_name(win, tx, Naming::Rename(id), &seed.unwrap_or_default());
                });
            let delete = button(tokens, Button::Ghost, "Delete", None)
                .key(format!("{KEY}.delete"))
                .on_press_tx(move |win: &mut Win, _: &mut Tx| {
                    dispatch(win, Msg::Grid(GridMsg::Forget(id)));
                    naming(win, Naming::Idle);
                });
            let line = row()
                .key(format!("{KEY}.offered"))
                .h(ITEM)
                .gap(4.0)
                .center_y()
                .child(text(name).font(LABEL).color(colour(tokens.dim)).truncate())
                .child(spacer())
                .child(rename)
                .child(delete);
            Some(line)
        }
    }
}

/// Every pane kind the grid shows, Settings and Themes aside: they open in a window.
fn kinds() -> impl Iterator<Item = PaneKind> {
    let apart = [SHOW_SETTINGS, SHOW_THEMES];
    let shown = PANES
        .iter()
        .filter(move |(command, _)| !apart.contains(&command.id));
    shown.map(|(_, kind)| *kind)
}

/// The panes of `kind` this window draws.
fn drawn(hub: &Hub, window: WindowId, kind: PaneKind) -> Vec<PaneId> {
    let grid = &hub.model.grid;
    let Some(shown) = grid.window(window) else {
        return Vec::new();
    };
    let docked = shown.layout.docked().map(|(pane, _)| pane);
    docked
        .filter(|pane| grid.kind(*pane) == Some(kind))
        .collect()
}

/// On, a pane is placed as its Show command places it; off, every one of its kind here is
/// closed as its bar's cross closes it.
fn flip(win: &mut Win, tx: &mut Tx, kind: PaneKind) {
    let window = win.window;
    let panes = win.hub.try_borrow().map(|hub| drawn(&hub, window, kind));
    let panes = panes.unwrap_or_default();
    if panes.is_empty() {
        show_pane(win, tx, kind);
    }
    for pane in panes {
        act(win, tx, Act::Close(pane));
    }
}

fn pane_item(hub: &Hub, window: WindowId, kind: PaneKind) -> El {
    let tokens = &hub.model.tokens;
    let on = kind == PaneKind::Chat || !drawn(hub, window, kind).is_empty();
    let key = format!("{KEY}.pane.{}", kind.slug());
    let ink = if on { tokens.fg } else { tokens.faint };
    let state = match (kind, on) {
        (PaneKind::Chat, _) => "always on",
        (_, true) => "on",
        (_, false) => "off",
    };
    let item = row()
        .key(key.clone())
        .w((WIDTH - 16.0) / 2.0)
        .h(ITEM)
        .px(5.0)
        .gap(6.0)
        .center_y()
        .radius(RADIUS_CHIP.of(tokens))
        .label(format!("{}, {state}", kind.title()))
        .child(glyph(icon_of(kind), 11.0, ink))
        .child(
            text(kind.title())
                .font(LABEL)
                .color(colour(ink))
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .child(toggle(tokens, &format!("{key}.switch"), on));
    match kind {
        PaneKind::Chat => item.help("Chat is always on"),
        _ => item
            .hover(colour(HOVER.of(tokens)))
            .cursor(Cursor::Pointer)
            .on_press_tx(move |win: &mut Win, tx: &mut Tx| flip(win, tx, kind)),
    }
}

fn side_switch(tokens: &Tokens, side: ChatSide) -> El {
    let on = match side {
        ChatSide::Left => 0,
        ChatSide::Right => 1,
    };
    let pick = move |win: &mut Win, index: usize| {
        if index != on {
            dispatch(win, Msg::Command(Route::Model, Run::new(FLIP_CHAT)));
        }
    };
    row()
        .h(21.0)
        .gap(6.0)
        .center_y()
        .child(text("Chat on").font(LABEL).color(colour(tokens.dim)))
        .child(spacer())
        .child(segmented(
            tokens,
            &format!("{KEY}.side"),
            &["Left", "Right"],
            on,
            pick,
        ))
}

/// The panel, hung under the Panes button while this window's grip says it is open. A
/// window with no Panes button, a pane's own, draws none.
pub fn layer(win: &Win, hub: &Hub, cx: &mut Cx<Win>, layer: &mut Vec<El>) {
    let anchor = cx.window_layout(BUTTON);
    let grid_at = cx.window_layout(view::KEY);
    let now = grip_of(hub, win.window);
    if !now.picking || anchor.is_none() {
        return;
    }
    let (tokens, grid) = (&hub.model.tokens, &hub.model.grid);
    let room = match grid_at {
        Some([_, _, w, h]) if w > 0.0 && h > 0.0 => Size { w, h },
        _ => grid.room(win.window),
    };
    let keys = command_box::keymap(hub);
    let all: Vec<PaneKind> = kinds().collect();
    let pairs = all.chunks(2).map(|pair| {
        let items = pair.iter().map(|kind| pane_item(hub, win.window, *kind));
        row().gap(4.0).children(items)
    });
    let line = el().h(1.0).w_full().fill(colour(LINE_MENU.of(tokens)));
    let surface = popover_shadow(
        col()
            .key(KEY)
            .role("grid")
            .label("Panes and layouts")
            .focus_group()
            .w(WIDTH)
            .padding(6.0)
            .gap(3.0)
            .radius(RADIUS_MENU.of(tokens))
            .fill(colour(POPOVER.of(tokens))),
        tokens,
    )
    .child(section(tokens, "Layouts", None, None))
    .child(layouts(hub, win.window, room, &keys))
    .children(asking(win, hub, now.naming))
    .child(side_switch(tokens, grid.side))
    .child(line)
    .child(section(tokens, "Panes", None, None))
    .children(pairs)
    .paint_order(OVER_ALL)
    .anchored(BUTTON, Side::Bottom, Align::End, 4.0)
    .modal(true, close);
    layer.push(surface);
}

#[cfg(test)]
mod tests {
    use yd_core::grid::saved::SAVED;

    use super::*;

    /// Every saved layout has a picture and every kept number a command to show it by.
    #[test]
    fn every_layout_and_kept_number_has_a_way_in() {
        assert_eq!(LAYOUTS.len(), SAVED.len());
        let numbers = FIRST_OWN..FIRST_OWN.saturating_add(MAX_OWN);
        assert!(numbers.map(OwnId).all(|id| own_command(id).is_some()));
        assert_eq!(super::super::OWN.len(), usize::from(MAX_OWN));
    }
}
