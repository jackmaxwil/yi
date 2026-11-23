//! Peek: Space shows the thing under the pointer in a card over the window, and no pane.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rune::{Cx, El, Key, KeyEvent, Picked, PointerEvent, Shadow, Tx, col, el, row, spacer, text};
use yd_core::address::thing::{Thing, lines};
use yd_core::address::uri::Address;
use yd_core::feed::{FILES, FeedEvent, SourceCmd};
use yd_core::ids::{RowId, WindowId};
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_core::row::Row;
use yd_core::tokens::Tokens;

use crate::menu::command_box::borrow;
use crate::menu::context;
use crate::overlay::boxes;
use crate::overlay::pick::{self, Found};
use crate::pane::{Scene, Stage};
use crate::theme::look::{BODY, CODE_LINE, META, colour};
use crate::theme::scale::{LINE_DIVIDER, POPOVER, RADIUS_MENU, RING_MENU, SHADOW_MENU};
use crate::win::{Hub, Win, dispatch};

pub const SCENES: &[Scene] = &[Scene {
    name: "peek/file",
    stage: Stage::View(context::scene),
    setup: file,
}];

pub const KEY: &str = "window.peek";
const SCRIM: &str = "window.peek-scrim";
const WIDTH: f32 = 460.0;
/// The most lines the card shows; its foot says how many there are.
pub const LINES: usize = 24;
const LINE: f32 = 16.0;
const HEAD: f32 = 26.0;
const FOOT: f32 = 22.0;
const EDGE: f32 = 4.0;
/// How far over and under the pointer the card keeps clear, as a row's height would.
const REACH: f32 = 10.0;
/// The card paints over every pane and the grid's own menus.
const OVER_ALL: i16 = 8;

/// Where the pointer last was in a window, kept because a key press does not say.
#[derive(Debug, Default, Clone, Copy)]
pub struct Pointer(Option<[f32; 2]>);

/// One window's open peek: what it is of, the element that stands for it, and where the
/// pointer was when that was found under it.
#[derive(Debug, Clone, PartialEq)]
pub struct Open {
    pub key: String,
    pub thing: Thing,
    pub at: Option<[f32; 2]>,
}

#[derive(Debug, Default)]
pub struct Peeks(pub BTreeMap<WindowId, Open>);

/// The pointer moved in the window. Nothing drawn reads where, so no frame follows.
pub fn moved(win: &mut Win, at: PointerEvent, tx: &mut Tx) {
    if let Some(pointer) = win.local.get_mut::<Pointer>(None) {
        pointer.0 = Some([at.window_x, at.window_y]);
    }
    tx.live_only();
}

pub fn open(hub: &Hub, window: WindowId) -> Option<&Open> {
    hub.panes.get::<Peeks>(None)?.0.get(&window)
}

/// Closes this window's peek; false when none was open.
pub fn close(win: &mut Win) -> bool {
    let window = win.window;
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return false;
    };
    let peeks = hub.panes.get_mut::<Peeks>(None);
    peeks.is_some_and(|peeks| peeks.0.remove(&window).is_some())
}

fn show(win: &mut Win, found: Found, at: Option<[f32; 2]>) {
    let window = win.window;
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return;
    };
    let (key, thing) = (found.key, found.thing);
    let peeks = hub.panes.get_mut::<Peeks>(None);
    peeks.map(|peeks| peeks.0.insert(window, Open { key, thing, at }));
}

/// Space: an open peek closes. Else the thing under the pointer is peeked, and with the
/// pointer on none, the thing at `cursor`: the row a list's arrow keys stand on.
pub fn toggle(win: &mut Win, tx: &mut Tx, cursor: Option<String>) {
    if tx.key_repeat() || close(win) {
        return;
    }
    let pointer = win.local.get::<Pointer>(None).and_then(|pointer| pointer.0);
    let look = move |win: &mut Win, picked: Option<Picked>, _: &mut Tx| {
        let found = win.hub.try_borrow().ok().and_then(|hub| {
            let under = picked.and_then(|picked| pick::found(&hub, &picked));
            // A pane's own ground stands for the pane, which is on screen already.
            let under = under.filter(|found| found.thing.kind() != Kind::Pane);
            under.map(|found| (found, pointer)).or_else(|| {
                let thing = pick::thing_at(&hub, cursor.as_deref()?)?;
                let key = cursor?;
                Some((Found { key, thing }, None))
            })
        });
        if let Some((found, at)) = found {
            show(win, found, at);
        }
    };
    match pointer {
        Some([x, y]) => tx.element_at_in(boxes::BASE, x, y, look),
        None => look(win, None, tx),
    }
}

/// Space, heard at the window's root before the control with the keys: it closes an open
/// peek, peeks on a control that stands for a thing, and is left to press any other.
pub fn key(win: &mut Win, event: &KeyEvent, tx: &mut Tx) -> bool {
    let mods = event.mods;
    let space = matches!(&event.key, Key::Text(typed) if typed == " ");
    if !space || mods.cmd || mods.ctrl || mods.alt || mods.shift {
        return false;
    }
    // Invariant: a held Space repeats, and a repeat opens and closes nothing.
    if !tx.key_repeat() && close(win) {
        return true;
    }
    let focused = tx.focused().map(str::to_owned);
    let thing = focused.filter(|key| {
        let hub = win.hub.try_borrow().ok();
        let thing = hub.and_then(|hub| pick::thing_at(&hub, key));
        thing.is_some_and(|thing| !matches!(thing.kind(), Kind::Pane | Kind::Tab))
    });
    let Some(key) = thing else { return false };
    toggle(win, tx, Some(key));
    true
}

/// What the card says: its lines, the number its first line wears when they are a file's,
/// and how many lines there are in all.
struct Said<'a> {
    rows: Vec<&'a str>,
    first: Option<usize>,
    all: usize,
    /// The file to ask the files source for: its text has not been read.
    unread: Option<&'a str>,
}

fn said<'a>(hub: &'a Hub, thing: &'a Thing) -> Said<'a> {
    let address = &thing.address;
    let plain = |words: &'a str| {
        let all = words.lines().count();
        let rows = words.lines().take(LINES).collect();
        let (first, unread) = (None, None);
        Said {
            rows,
            first,
            all,
            unread,
        }
    };
    if !matches!(address.kind(), Kind::File | Kind::Lines) {
        return plain(thing.excerpt.as_deref().unwrap_or(address.id()));
    }
    let id = RowId::new(format!("file:{}", address.id()));
    let read = hub
        .model
        .tables
        .get(FILES)
        .and_then(|files| files.find(&id));
    let Some(words) = read.and_then(|read| read.words("text").or(read.words("error"))) else {
        let unread = Some(address.id());
        return Said {
            unread,
            ..plain("Reading…")
        };
    };
    let from = address
        .part_text()
        .and_then(lines)
        .map_or(1, |(from, _)| from);
    let skip = usize::try_from(from).unwrap_or(1).saturating_sub(1);
    let rows: Vec<&str> = words.lines().skip(skip).take(LINES).collect();
    let (first, all) = (Some(skip.saturating_add(1)), words.lines().count());
    Said {
        rows,
        first,
        all,
        unread: None,
    }
}

/// Asks the files source for the text of `path` once, when this is laid out.
fn asked(path: &str) -> El {
    let file = PathBuf::from(path);
    el().key(format!("{KEY}.ask.{path}"))
        .size(0.0, 0.0)
        .on_layout_tx(move |win: &mut Win, _, _: &mut Tx| {
            dispatch(win, Msg::Source(FILES, SourceCmd::Read(file.clone())));
        })
}

fn line(tokens: &Tokens, at: usize, number: Option<usize>, words: &str) -> El {
    let gutter = number.map(|number| {
        let said = text(number.to_string())
            .font(CODE_LINE)
            .color(colour(tokens.faint));
        row().w(34.0).pr(8.0).child(spacer()).child(said)
    });
    let words = text(words.replace('\t', "    "))
        .font(CODE_LINE)
        .color(colour(tokens.fg));
    row()
        .key(format!("{KEY}.line.{at}"))
        .w_full()
        .h(LINE)
        .center_y()
        .children(gutter)
        .child(words.truncate().min_w(0.0))
}

fn card(tokens: &Tokens, open: &Open, said: &Said<'_>, at: [f32; 2]) -> El {
    let rule = colour(LINE_DIVIDER.of(tokens));
    let faint = |words: String| text(words).font(META).color(colour(tokens.faint));
    let head = row()
        .key(format!("{KEY}.head"))
        .w_full()
        .h(HEAD)
        .px(10.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], rule)
        .child(
            text(open.thing.label.as_str())
                .font(BODY)
                .color(colour(tokens.fg))
                .truncate(),
        );
    let number = |at: usize| said.first.map(|first| first.saturating_add(at));
    let rows = said.rows.iter().enumerate();
    let rows = rows.map(|(at, words)| line(tokens, at, number(at), words));
    let last = said
        .first
        .unwrap_or(1)
        .saturating_add(said.rows.len())
        .saturating_sub(1);
    let count = match said.first {
        Some(first) if said.unread.is_none() => format!("lines {first}–{last} of {}", said.all),
        None if said.all > said.rows.len() => format!("{} of {} lines", said.rows.len(), said.all),
        _ => String::new(),
    };
    let foot = row()
        .key(format!("{KEY}.foot"))
        .w_full()
        .h(FOOT)
        .px(10.0)
        .center_y()
        .border_sides([1.0, 0.0, 0.0, 0.0], rule)
        .child(faint("space to close".to_owned()))
        .child(spacer())
        .child(faint(count).key(format!("{KEY}.count")));
    col()
        .key(KEY)
        .absolute(at[0], at[1])
        .w(WIDTH)
        .radius(RADIUS_MENU.of(tokens))
        .fill(colour(POPOVER.of(tokens)))
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
        .child(head)
        .child(col().w_full().py(4.0).px(2.0).children(rows))
        .child(foot)
        .children(said.unread.map(asked))
}

/// This window's peek over `layer`: a ground that closes it on a press, then the card,
/// under the pointer or the element it is of where there is room, and over where not.
pub fn layer(win: &Win, hub: &Hub, cx: &mut Cx<Win>, layer: &mut Vec<El>) {
    let Some(open) = open(hub, win.window) else {
        return;
    };
    let said = said(hub, &open.thing);
    let tall = HEAD + FOOT + 8.0 + LINE * said.rows.len() as f32;
    let room = cx.layout_size(SCRIM).unwrap_or([f32::MAX; 2]);
    // Incident: Rune's read of a Files row was 8 points under where the row is drawn, so
    // a thing found under the pointer hangs the card from the pointer, not from its element.
    let read = cx.window_layout(&open.key).map(|[x, y, _, h]| [x, y, h]);
    let [x, y, h] = match open.at {
        Some([x, y]) => [x, y - REACH, 2.0 * REACH],
        None => read.unwrap_or([EDGE, EDGE, 0.0]),
    };
    let under = y + h + EDGE;
    let fits = under + tall + EDGE <= room[1];
    let top = if fits { under } else { y - tall - EDGE };
    let at = [x.min(room[0] - WIDTH - EDGE).max(EDGE), top.max(EDGE)];
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
    layer.push(scrim);
    layer.push(card(&hub.model.tokens, open, &said, at));
}

/// The file the scene peeks, longer than the card: its foot says how much is shown.
const PEEKED: &str = "src/middleware/rate_limit.rs";

/// The Changes pane over the mockup's thread, a peek open on the rate limiter's file.
fn file(hub: &mut Hub) {
    borrow(hub, &["chat/mockup", "changes/mockup"]);
    let Ok(address) = Address::new(Kind::File, PEEKED) else {
        return;
    };
    let fields = (0..12).map(|n| format!("    /// Field {n} of the window.\n    f{n}: u32,\n"));
    let text: String = ["pub struct Window {\n".to_owned()]
        .into_iter()
        .chain(fields)
        .chain(["}\n".to_owned()])
        .collect();
    let row = Row::new(format!("file:{PEEKED}")).text("text", text);
    hub.model.tables.apply(FILES, FeedEvent::Upsert(row));
    let open = Open {
        key: String::new(),
        thing: Thing::at(&hub.model, address),
        at: Some([372.0, 118.0]),
    };
    if let Some(peeks) = hub.panes.get_mut::<Peeks>(None) {
        peeks.0.insert(WindowId(0), open);
    }
}
