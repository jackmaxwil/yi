//! The right-click menu, built from the items of the thing's kind (record 4.10): one menu at
//! the pointer, the same order on anything.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use rune::{
    Cursor, Cx, El, Icon, Picked, PointerEvent, ScrollSpec, Shadow, TextStyle, Tx, col, el, row,
    spacer, text,
};
use yd_core::address::attach::AttachMsg;
use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::command::by_kind::{
    self, ASK, ASKS, ATTACH, ENDS, FOOTER, Hint, Icon as Named, Item, TOP, items, turn_of,
};
use yd_core::command::table::keycap;
use yd_core::command::table::{Run, Target};
use yd_core::ids::{CommandId, PaneId, WindowId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;

use crate::chrome::tabs;
use crate::grid_view::aimed;
use crate::grid_view::view::alone;
use crate::kit::button::PRESS_SCALE;
use crate::kit::chip::{Chip, chip};
use crate::kit::icons::{SEARCH, glyph};
use crate::line_icon;
use crate::menu::command_box::{self, Ran, borrow, table};
use crate::overlay::{boxes, pick};
use crate::pane::{Scene, Stage};
use crate::shown;
use crate::theme::look::{BODY, CODE, LABEL, META, colour};
use crate::theme::scale::{
    HOVER, LINE_MENU, MENU, RADIUS_CHIP, RADIUS_MENU, RING_MENU, SEMIBOLD, SHADOW_MENU,
};
use crate::win::{Hub, Win, dispatch};

pub const SCENES: &[Scene] = &[Scene {
    name: "context-menu/mockup",
    stage: Stage::View(scene),
    setup: mockup,
}];

pub const KEY: &str = "window.context-menu";
const HEAD: &str = "window.context-menu-head";
const SCRIM: &str = "window.context-menu-scrim";
const WIDTH: f32 = 244.0;
/// How far from the window's right edge the menu's corner may open (record 4.10).
const REACH: f32 = 252.0;
/// The menu's height less its rows: the record's 380 with eleven rows of 22.
const CHROME: f32 = 138.0;
const EDGE: f32 = 4.0;
const ITEM: f32 = 22.0;
/// The menu paints over every pane and the grid's own menus.
const OVER_ALL: i16 = 8;

/// One window's open menu: where it was opened and what it is about.
#[derive(Debug, Clone, PartialEq)]
pub struct Open {
    pub at: [f32; 2],
    pub thing: Thing,
    /// What the last pick came to when nothing carried it out.
    pub said: Option<String>,
}

#[derive(Debug, Default)]
pub struct Menus(pub BTreeMap<WindowId, Open>);

/// Opens the menu in `window`; one already open there is replaced.
pub fn open_in(hub: &mut Hub, window: WindowId, at: [f32; 2], thing: Thing) {
    if let Some(menus) = hub.panes.get_mut::<Menus>(None) {
        menus.0.insert(
            window,
            Open {
                at,
                thing,
                said: None,
            },
        );
    }
}

/// Closes this window's menu; false when none was open.
pub fn close(win: &mut Win) -> bool {
    let window = win.window;
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return false;
    };
    let gone = hub
        .panes
        .get_mut::<Menus>(None)
        .and_then(|menus| menus.0.remove(&window));
    gone.is_some()
}

/// The pane a key is drawn in, as a thing: what the menu is about when the key's own thing is
/// not known. Nothing for a key outside every pane, such as the shell's.
pub fn pane_thing(key: &str) -> Option<Thing> {
    let root = key.split('.').next()?;
    let (slug, id) = root.rsplit_once('-')?;
    let kind = PaneKind::ALL.iter().find(|kind| kind.slug() == slug)?;
    let pane = PaneId(id.parse().ok()?);
    Some(Thing {
        label: kind.title().to_owned(),
        address: Address::new(Kind::Pane, pane.0.to_string()).ok()?,
        excerpt: None,
    })
}

/// A secondary press on the element keyed `key`: the menu for `thing`, else for the pane the
/// key is in. A key in no pane opens nothing and leaves the system's menu to the press.
pub fn secondary(win: &mut Win, at: [f32; 2], key: &str, thing: Option<Thing>) -> bool {
    let Some(thing) = thing.or_else(|| pane_thing(key)) else {
        return false;
    };
    let window = win.window;
    if let Ok(mut hub) = win.hub.try_borrow_mut() {
        open_in(&mut hub, window, at, thing);
    }
    true
}

/// Opens the menu for what Rune picked under the pointer: its thing as the one lookup finds
/// it, else the pane its key is in.
fn open_at(win: &mut Win, at: [f32; 2], picked: &Picked) {
    let found = win.hub.try_borrow().ok();
    let found = found.and_then(|hub| pick::found(&hub, picked));
    match (found, picked.key()) {
        (Some(found), _) => secondary(win, at, &found.key, Some(found.thing)),
        (None, Some(key)) => secondary(win, at, key, None),
        (None, None) => false,
    };
}

/// A secondary press anywhere in the window. Invariant: the overlay's layers lie over the
/// window's body, so one pick looks beneath them and the other serves a window with none.
pub fn press(_win: &mut Win, event: PointerEvent, tx: &mut Tx) {
    let at = [event.window_x, event.window_y];
    let open = move |win: &mut Win, picked: Option<Picked>, _: &mut Tx| {
        let layer = |picked: &Picked| picked.path().split('/').any(|key| key == boxes::KEY);
        if let Some(picked) = picked.filter(|picked| !layer(picked)) {
            open_at(win, at, &picked);
        }
    };
    tx.element_at(at[0], at[1], open);
    let beneath = move |win: &mut Win, picked: Option<Picked>, _: &mut Tx| {
        if let Some(picked) = picked {
            open_at(win, at, &picked);
        }
    };
    tx.element_at_in(boxes::BASE, at[0], at[1], beneath);
}

/// What the header calls the thing: a line of a file by its number, else the thing's label.
pub fn heading(thing: &Thing) -> String {
    let address = &thing.address;
    let lines = address.part_text().and_then(|part| {
        let (first, last) = part.split_once('-').unwrap_or((part, part));
        Some((first.strip_prefix('L')?, last.strip_prefix('L')?))
    });
    match (address.kind(), lines) {
        (Kind::Lines, Some((first, last))) => {
            let name = address.id().rsplit('/').next().unwrap_or(address.id());
            if first == last {
                format!("{name} line {first}")
            } else {
                format!("{name} lines {first}–{last}")
            }
        }
        _ => thing.label.clone(),
    }
}

fn attached(hub: &Hub, thing: &Thing) -> bool {
    let things = &hub.model.attached.things;
    things.iter().any(|kept| kept.address == thing.address)
}

/// The words at an item's right: its key, its own words, or the turn that wrote the thing.
fn hint(win: &Win, hub: &Hub, item: &Item, thing: &Thing) -> Option<String> {
    match item.hint {
        Hint::None => None,
        Hint::Said(words) => Some(words.to_owned()),
        Hint::Key => table(hub).find(item.command)?.key.map(keycap),
        Hint::Chosen => {
            let grid = &hub.model.grid;
            let pane = by_kind::pane_named(&thing.address)?;
            let said = || {
                let (_, kind, word) = by_kind::word(grid, item.command, &thing.address)?;
                Some(hub.model.settings.opens().get(&kind) == Some(&word))
            };
            let chosen = by_kind::keeps(grid, item.command, pane).or_else(said)?;
            chosen.then(|| "✓".to_owned())
        }
        Hint::Turn => {
            let thread = shown::thread(hub, win.window)?;
            let turn = turn_of(thread, thing.address.id())?;
            Some(format!("turn {turn}"))
        }
    }
}

/// Closes the menu, then does what was picked to the thing. A pick nothing carries out opens
/// the menu again saying so, as a button does.
fn pick(win: &mut Win, tx: &mut Tx, command: CommandId, words: Option<&str>) {
    let open = win.hub.try_borrow().ok().and_then(|hub| {
        let menus = hub.panes.get::<Menus>(None)?;
        menus.0.get(&win.window).cloned()
    });
    close(win);
    let Some(Open { at, thing, .. }) = open else {
        return;
    };
    if command == ATTACH {
        let thing = match win.hub.try_borrow() {
            Ok(hub) => crate::overlay::pick::filled(&hub, thing),
            Err(_) => thing,
        };
        dispatch(win, Msg::Attach(AttachMsg::Toggle(thing)));
        return;
    }
    let mut run = Run::new(command).on(Target::Thing(thing.address.clone()));
    run.text = words.map(str::to_owned);
    let title = match command_box::run(win, tx, run) {
        Ran::Done => return,
        Ran::NotBuilt(title) => Some(title),
        Ran::Unknown => None,
    };
    let window = win.window;
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    let title = title.or_else(|| items_titled(command)).unwrap_or(command.0);
    let said = Some(format!("{title} is not available yet"));
    if let Some(menus) = hub.panes.get_mut::<Menus>(None) {
        menus.0.insert(window, Open { at, thing, said });
    }
}

/// The kind's own rows the menu offers for the thing it is open on.
fn middle(hub: &Hub, open: &Open) -> Vec<&'static Item> {
    let address = &open.thing.address;
    let offered = |item: &&Item| aimed::offered(hub, item.command, address);
    items(open.thing.kind()).iter().filter(offered).collect()
}

/// The words of the item that runs `command`, on any kind's menu.
fn items_titled(command: CommandId) -> Option<&'static str> {
    let all = TOP.iter().chain(ENDS.iter().copied().flatten());
    let kinds = Kind::ALL.iter().flat_map(|kind| items(*kind).iter());
    let item = all.chain(kinds).find(|item| item.command == command)?;
    Some(item.label)
}

fn icon(named: Named) -> &'static Icon {
    match named {
        Named::Plus => &tabs::PLUS,
        Named::Chat => &tabs::CHAT,
        Named::Branch => &tabs::BRANCH,
        Named::Search => &SEARCH,
        Named::Reload => &RELOAD,
        Named::Target => &tabs::TARGET,
        Named::Folder => &tabs::FOLDER,
        Named::Diff => &tabs::DIFF,
        Named::Stop => &STOP,
        Named::Sliders => &tabs::SLIDERS,
        Named::File => &tabs::FILE,
        Named::Term => &tabs::TERM,
        Named::Monitor => &tabs::MONITOR,
        Named::Edit => &EDIT,
        Named::Check => &CHECK,
        Named::Eye => &EYE,
        Named::Shield => &tabs::SHIELD,
        Named::Panel => &tabs::PANEL,
        Named::Out => &tabs::OUT,
        Named::X => &tabs::X,
    }
}

static RELOAD: Icon = line_icon!(
    "reload",
    r#"<path d="M20 12a8 8 0 1 1-2.600-5.900M20 4v5h-5"/>"#
);
static STOP: Icon = line_icon!(
    "stop",
    r#"<rect x="7" y="7" width="10" height="10" rx="2" fill="currentColor" stroke="none"/>"#
);
static EDIT: Icon = line_icon!("edit", r#"<path d="M4 20h4L19 9l-4-4L4 16z"/>"#);
static CHECK: Icon = line_icon!("check", r#"<path d="m5 12.5 4.5 4.5L19 7.5"/>"#);
static EYE: Icon = line_icon!(
    "eye",
    r#"<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>"#
);

/// What one menu draws from: the window, the hub and the open menu.
struct Drawn<'a> {
    win: &'a Win,
    hub: &'a Hub,
    open: &'a Open,
}

impl Drawn<'_> {
    fn item(&self, item: &Item) -> El {
        let tokens = &self.hub.model.tokens;
        let thing = &self.open.thing;
        let label = if item.command == ATTACH && attached(self.hub, thing) {
            "Remove from chat"
        } else {
            item.label
        };
        let hint = hint(self.win, self.hub, item, thing)
            .map(|words| text(words).font(META).color(colour(tokens.faint)));
        let item = *item;
        row()
            .key(format!("{KEY}.row.{}", item.command.0))
            .w_full()
            .h(ITEM)
            .px(8.0)
            .gap(8.0)
            .center_y()
            .radius(RADIUS_CHIP.of(tokens))
            .hover(colour(HOVER.of(tokens)))
            .cursor(Cursor::Pointer)
            .press_scale(PRESS_SCALE)
            .child(glyph(icon(item.icon), 11.0, tokens.dim))
            .child(
                text(label)
                    .font(LABEL)
                    .color(colour(tokens.fg))
                    .truncate()
                    .min_w(0.0),
            )
            .child(spacer())
            .children(hint)
            .on_press_tx(move |win: &mut Win, tx: &mut Tx| pick(win, tx, item.command, None))
    }

    fn rule(&self) -> El {
        let ink = LINE_MENU.of(&self.hub.model.tokens);
        let line = el().h(1.0).w_full().fill(colour(ink));
        col().w_full().px(8.0).py(4.0).child(line)
    }

    fn head(&self) -> El {
        let tokens = &self.hub.model.tokens;
        let thing = &self.open.thing;
        let label = text(heading(thing))
            .font(TextStyle {
                weight: SEMIBOLD,
                ..BODY
            })
            .color(colour(tokens.fg))
            .truncate();
        let address = text(thing.address.to_string())
            .font(META.family(CODE))
            .color(colour(tokens.faint))
            .truncate();
        col()
            .key(HEAD)
            .w_full()
            .pt(5.0)
            .px(8.0)
            .pb(6.0)
            .child(label)
            .child(address)
    }

    fn asks(&self) -> El {
        let tokens = &self.hub.model.tokens;
        let ask = |words: &&'static str| {
            let words: &'static str = words;
            chip(tokens, Chip::Pill, words, None, false)
                .key(format!("{KEY}.ask.{words}"))
                .on_press_tx(move |win: &mut Win, tx: &mut Tx| {
                    pick(win, tx, ASK, Some(words));
                })
        };
        let asks = row()
            .w_full()
            .px(6.0)
            .pb(6.0)
            .gap(4.0)
            .flex_wrap()
            .min_w(0.0);
        asks.children(ASKS.iter().map(ask))
    }

    fn card(&self, at: [f32; 2], room: f32) -> El {
        let tokens = &self.hub.model.tokens;
        let middle = middle(self.hub, self.open);
        let mut card = col()
            .key(KEY)
            .absolute(at[0], at[1])
            .w(WIDTH)
            .max_h(room)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .padding(4.0)
            .radius(RADIUS_MENU.of(tokens))
            .fill(colour(MENU.of(tokens)))
            .paint_order(OVER_ALL)
            .box_shadow(Shadow {
                spread: 1.0,
                color: colour(RING_MENU.of(tokens)),
                ..Shadow::default()
            })
            .box_shadow(Shadow {
                y: 18.0,
                blur: 44.0,
                spread: -10.0,
                color: colour(SHADOW_MENU.of(tokens)),
                ..Shadow::default()
            })
            .on_background_press_tx(|_: &mut Win, _: &mut Tx| {})
            .child(self.head())
            .child(self.asks())
            .children(TOP.iter().map(|item| self.item(item)));
        if !middle.is_empty() {
            card = card.child(self.rule());
            card = card.children(middle.into_iter().map(|item| self.item(item)));
        }
        for group in ENDS {
            card = card.child(self.rule());
            card = card.children(group.iter().map(|item| self.item(item)));
        }
        let said = self.open.said.as_ref().map(|said| {
            let line = text(said.as_str()).font(META).color(colour(tokens.dim));
            col()
                .key(format!("{KEY}.said"))
                .w_full()
                .px(8.0)
                .pt(4.0)
                .child(line.wrap())
        });
        let footer = text(FOOTER).font(META).color(colour(tokens.faint)).wrap();
        card.children(said)
            .child(col().w_full().px(8.0).pt(4.0).pb(3.0).child(footer))
    }
}

/// The menu at its place in the window, and the scrim that closes it on a press outside.
pub fn view(win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> Vec<El> {
    let Some(open) = hub
        .panes
        .get::<Menus>(None)
        .and_then(|m| m.0.get(&win.window))
    else {
        return Vec::new();
    };
    // Incident: the menu kept the record's 380 points clear below it whatever its rows, and
    // a bar's seventeen ran 128 points off the bottom of the window.
    let groups = ENDS.iter().map(|group| group.len()).sum::<usize>();
    let rows = TOP.len() + middle(hub, open).len() + groups;
    let tall = CHROME + ITEM * rows as f32;
    let size = cx.layout_size(SCRIM);
    let room = |axis: usize| size.and_then(|size| size.get(axis).copied());
    let place = |at: f32, axis: usize, reach: f32| {
        let far = room(axis).map_or(f32::MAX, |room| room - reach);
        at.min(far).max(EDGE)
    };
    let at = [place(open.at[0], 0, REACH), place(open.at[1], 1, tall)];
    let room = room(1).map_or(f32::MAX, |room| room - 2.0 * EDGE);
    let shut = |win: &mut Win, _: &mut Tx| {
        close(win);
    };
    let scrim = el()
        .key(SCRIM)
        .absolute(0.0, 0.0)
        .w_full()
        .h_full()
        .paint_order(OVER_ALL)
        .on_background_press_tx(shut)
        .on_secondary_press_local(move |win: &mut Win, _: PointerEvent, tx: &mut Tx| shut(win, tx));
    let drawn = Drawn { win, hub, open };
    vec![scrim, drawn.card(at, room)]
}

/// The diff line the mockup's frame 14 right-clicks: line 18 of the new rate limiter.
const LINE: (&str, &str) = ("src/middleware/rate_limit.rs", "L18");

/// Frame 14: the Changes pane over the mockup's thread, the menu open on diff line 18.
fn mockup(hub: &mut Hub) {
    borrow(hub, &["chat/mockup", "changes/mockup"]);
    let Ok(address) = Address::new(Kind::Lines, LINE.0) else {
        return;
    };
    let thing = Thing {
        label: format!("rate_limit.rs:{}", LINE.1.trim_start_matches('L')),
        address: address.part(LINE.1),
        excerpt: None,
    };
    open_in(hub, WindowId(0), [372.0, 318.0], thing);
}

/// The Changes pane as its own mockup scene draws it, with the window's menus over it.
pub(crate) fn scene(win: &Win, hub: &Hub, cx: &mut Cx<Win>) -> El {
    let changes = hub
        .scenes
        .iter()
        .find(|scene| scene.name == "changes/mockup");
    let base = match changes.map(|scene| scene.stage) {
        Some(Stage::View(view)) => view(win, hub, cx),
        Some(Stage::Pane(kind)) => alone(win, hub, cx, kind),
        Some(Stage::App) | None => el().w_full().h_full(),
    };
    rune_kit::root(base, command_box::layer(win, hub, cx))
}
