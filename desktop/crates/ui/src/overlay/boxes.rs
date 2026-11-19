//! Context boxes: brackets, number tabs and labels over attached things, and the pick layer.

mod scenes;
#[cfg(test)]
mod tests;
pub mod tray;

use rune::{
    Cursor, Cx, El, Key, KeyEvent, PointerEvent, Shadow, TextStyle, Tx, col, el, row, text,
};
use yd_core::address::thing::lines;
use yd_core::address::uri::Address;
use yd_core::kind::Kind;
use yd_core::pane::PaneKind;
use yd_core::tokens::{Rgba, Tokens};

use crate::overlay::pick::{self, Context, Found, LAYER};
use crate::pane::Scene;
use crate::theme::look::{CODE, META, colour};
use crate::theme::scale::{CONTEXT_PREVIEW, CONTEXT_TAG, ON_CONTEXT, SHADOW_NEAR};
use crate::win::{Hub, Win};

pub const SCENES: &[Scene] = scenes::SCENES;
pub use scenes::mockup as attach_mockup;

/// The element [`over`] makes: everything it draws is keyed under it.
pub const KEY: &str = "overlay";
/// What the boxes are drawn over. Picking looks for elements inside it alone.
pub const BASE: &str = "overlay.base";
/// The layer of boxes. It takes no input.
pub const LAYER_BOXES: &str = "overlay.boxes";
/// The box under the pointer while picking.
pub const PREVIEW: &str = "overlay.preview";
/// The start of the key of the box round a pane while picking: `overlay.pane.<pane key>`.
pub const PANE_BOX: &str = "overlay.pane.";

/// A box is its element grown by this much on every side.
const GROW: f32 = 3.0;
/// A corner bracket's arm: how long and how thick.
const ARM: (f32, f32) = (12.0, 1.5);
/// The lines one box is drawn round at most; a longer range is boxed up to here.
const MAX_LINES: u32 = 512;
pub(crate) const NUMBER: TextStyle = TextStyle {
    weight: 700.0,
    ..META.lh(14.0)
};
pub(crate) const NAME: TextStyle = TextStyle {
    weight: 500.0,
    ..META.lh(14.0).family(CODE)
};

/// How a box is drawn: at rest, lit by its chip, faded beside a lit one, or as the preview
/// of what a press would pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Look {
    Rest,
    Lit,
    Faded,
    Preview,
}

/// The box keyed for the thing at `address`: `overlay.box.<address>`.
pub fn box_key(address: &Address) -> String {
    [KEY, ".box.", &address.to_string()].concat()
}

/// The part of a line's key before its `L<n>`: a range's other lines share it.
fn line_prefix(key: &str) -> Option<&str> {
    let is_line = |part: &str| lines(part).is_some() && !part.contains('-');
    let mut at = key.len();
    for part in key.rsplit('.') {
        let start = at.checked_sub(part.len())?;
        if is_line(part) {
            return key.get(..start.checked_sub(1)?);
        }
        at = start.checked_sub(1)?;
    }
    None
}

fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let (left, top) = (a[0].min(b[0]), a[1].min(b[1]));
    let right = (a[0] + a[2]).max(b[0] + b[2]);
    let bottom = (a[1] + a[3]).max(b[1] + b[3]);
    [left, top, right - left, bottom - top]
}

/// The part of `rect` inside `within`; none when they do not meet.
fn clip(rect: [f32; 4], within: [f32; 4]) -> Option<[f32; 4]> {
    let (left, top) = (rect[0].max(within[0]), rect[1].max(within[1]));
    let right = (rect[0] + rect[2]).min(within[0] + within[2]);
    let bottom = (rect[1] + rect[3]).min(within[1] + within[3]);
    (right > left && bottom > top).then_some([left, top, right - left, bottom - top])
}

/// A list draws a group's heading inside the group's first row, keyed `<list>.group.<row
/// id>`: the row itself is what is left under it.
fn below(heading: Option<[f32; 4]>, row: [f32; 4]) -> [f32; 4] {
    let Some(heading) = heading else {
        return row;
    };
    let top = (heading[1] + heading[3]).min(row[1] + row[3]);
    [row[0], top, row[2], row[1] + row[3] - top]
}

/// Where the window shows the thing found at `key`: its element, or for a range of lines
/// every line of it that is drawn, cut to the pane it is in. None while it is off screen.
fn rect_of(cx: &Cx<Win>, key: &str, address: &Address) -> Option<[f32; 4]> {
    // Invariant: every read is made on every run, found or not. Rune answers a read from
    // the run after the one that first makes it, so a read made late draws a frame late.
    let within = viewport(cx, key, address.kind());
    let group = key.split_once(".row.");
    let heading = group.and_then(|(list, id)| cx.window_layout(&[list, ".group.", id].concat()));
    let own = cx.window_layout(key);
    let range = address.part_text().and_then(lines);
    let range = range.filter(|_| address.kind() == Kind::Lines);
    let spread = range
        .zip(line_prefix(key))
        .and_then(|((first, last), prefix)| {
            let last = last.min(first.saturating_add(MAX_LINES));
            let drawn = (first..=last).filter_map(|n| cx.window_layout(&format!("{prefix}.L{n}")));
            drawn.reduce(union)
        });
    let rect = match spread {
        Some(lines) => lines,
        None => below(heading, own?),
    };
    match within {
        Some(within) => clip(rect, within),
        None => Some(rect),
    }
}

/// What cuts off an element of a pane: the list it scrolls in, `<list>.rows`, else the
/// pane's body. A tab sits in the pane's bar, above its body, and nothing cuts it off.
fn viewport(cx: &Cx<Win>, key: &str, kind: Kind) -> Option<[f32; 4]> {
    pick::pane_of(key).filter(|_| kind != Kind::Tab)?;
    let pane = key.split('.').next()?;
    let list = key.split_once(".row.").map_or(pane, |(list, _)| list);
    let rows = |of: &str| cx.window_layout(&[of, ".rows"].concat());
    let (list, all, body) = (rows(list), rows(pane), cx.window_layout(pane));
    list.or(all).or(body)
}

fn arm(ink: Rgba, at: (f32, f32), size: (f32, f32)) -> El {
    el().absolute(at.0, at.1)
        .size(size.0, size.1)
        .fill(colour(ink))
}

/// Eight arms, two a corner, inside a box `w` by `h`.
fn brackets(ink: Rgba, w: f32, h: f32) -> Vec<El> {
    let (long, thick) = (ARM.0.min(w / 2.0), ARM.1);
    let tall = ARM.0.min(h / 2.0);
    let (right, bottom) = (w - long, h - thick);
    vec![
        arm(ink, (0.0, 0.0), (long, thick)),
        arm(ink, (0.0, 0.0), (thick, tall)),
        arm(ink, (right, 0.0), (long, thick)),
        arm(ink, (w - thick, 0.0), (thick, tall)),
        arm(ink, (0.0, bottom), (long, thick)),
        arm(ink, (0.0, h - tall), (thick, tall)),
        arm(ink, (right, bottom), (long, thick)),
        arm(ink, (w - thick, h - tall), (thick, tall)),
    ]
}

fn shadow(tokens: &Tokens) -> Shadow {
    Shadow {
        x: 0.0,
        y: 2.0,
        blur: 6.0,
        spread: 0.0,
        color: colour(SHADOW_NEAR.of(tokens)),
        inset: false,
    }
}

/// What a box says above its top edge.
struct Says<'a> {
    /// The number on its tab; a preview has none.
    number: Option<usize>,
    name: &'a str,
    /// What a press would do, after a preview's name.
    hint: Option<&'static str>,
}

fn tags(tokens: &Tokens, key: &str, says: &Says<'_>, look: Look) -> Vec<El> {
    let context = tokens.context;
    let tab = says.number.map(|number| {
        row()
            .key(format!("{key}.number"))
            .absolute(6.0, -8.0)
            .h(14.0)
            .min_w(14.0)
            .px(4.0)
            .center()
            .radius(4.0)
            .fill(colour(context))
            .box_shadow(shadow(tokens))
            .child(
                text(number.to_string())
                    .font(NUMBER)
                    .color(colour(ON_CONTEXT.of(tokens))),
            )
    });
    let name = |ground: Rgba, ink: Rgba, at: (f32, f32)| {
        row()
            .key(format!("{key}.tag"))
            .absolute(at.0, at.1)
            .h(14.0)
            .px(5.0)
            .gap(6.0)
            .center_y()
            .radius(4.0)
            .fill(colour(ground))
            .box_shadow(shadow(tokens))
            .child(text(says.name).font(NAME).color(colour(ink)))
            .children(
                says.hint
                    .map(|hint| text(hint).font(NAME).color(colour(ink.alpha(0.55)))),
            )
    };
    let tag = match look {
        Look::Lit => Some(
            name(CONTEXT_TAG.of(tokens), context, (24.0, -8.0))
                .border(1.0, colour(context.alpha(0.35))),
        ),
        Look::Preview => Some(name(
            CONTEXT_PREVIEW.of(tokens),
            CONTEXT_TAG.of(tokens),
            (0.0, -17.0),
        )),
        Look::Rest | Look::Faded => None,
    };
    tab.into_iter().chain(tag).collect()
}

/// One box: `at` grown by three points, with corner brackets, a ring, and what it says.
fn frame(tokens: &Tokens, key: String, at: [f32; 4], says: &Says<'_>, look: Look) -> El {
    let ink = match look {
        Look::Preview => CONTEXT_PREVIEW.of(tokens),
        Look::Rest | Look::Lit | Look::Faded => tokens.context,
    };
    let (fill, ring, strength) = match look {
        Look::Rest => (0.06, 0.2, 1.0),
        Look::Lit => (0.06, 0.55, 1.0),
        Look::Faded => (0.06, 0.2, 0.4),
        Look::Preview => (0.0, 0.18, 1.0),
    };
    let (w, h) = (at[2] + GROW * 2.0, at[3] + GROW * 2.0);
    el().absolute(at[0] - GROW, at[1] - GROW)
        .size(w, h)
        .radius(3.0)
        .fill(colour(ink.alpha(fill)))
        .border(1.0, colour(ink.alpha(ring)))
        .opacity(strength)
        .children(brackets(ink, w, h))
        .children(tags(tokens, &key, says, look))
        .key(key)
}

/// The box design mode and mark-up draw themselves, keyed `key`: a context box round `at`
/// wearing `number`, its name tag shown when it has one.
pub fn numbered(tokens: &Tokens, key: String, at: [f32; 4], number: usize, name: &str) -> El {
    let says = Says {
        number: Some(number),
        name,
        hint: None,
    };
    let look = if name.is_empty() {
        Look::Rest
    } else {
        Look::Lit
    };
    frame(tokens, key, at, &says, look)
}

/// Whether the point is inside the box drawn round `rect`.
fn inside(rect: [f32; 4], x: f32, y: f32) -> bool {
    let (left, top) = (rect[0] - GROW, rect[1] - GROW);
    let (right, bottom) = (rect[0] + rect[2] + GROW, rect[1] + rect[3] + GROW);
    (left..=right).contains(&x) && (top..=bottom).contains(&y)
}

/// The pointer moved over the window: the box it is in lights its chip.
fn pointed(win: &mut Win, boxes: &[(Address, [f32; 4])], at: PointerEvent, tx: &mut Tx) {
    let over = |(_, rect): &&(Address, [f32; 4])| inside(*rect, at.window_x, at.window_y);
    let over = boxes.iter().rev().find(over);
    if !pick::point_box(win, over.map(|(address, _)| address)) {
        tx.live_only();
    }
}

/// The preview of what a press would pick, and what the press would do.
fn preview(hub: &Hub, cx: &Cx<Win>, found: &Found, origin: [f32; 2]) -> Option<El> {
    let [x, y, w, h] = rect_of(cx, &found.key, &found.thing.address)?;
    let attached = &hub.model.attached;
    let pane = pick::pane_of(&found.key).map(|(kind, _)| kind);
    let page = pane == Some(PaneKind::Web) && found.thing.kind() == Kind::Pane;
    let hint = match attached.number(&found.thing.address) {
        Some(_) => "click to remove",
        None if attached.is_full() => "twelve attached: remove one first",
        // Invariant: Yi's browser link answers no call, so a live page's element is never read.
        None if page => "click to attach the page · its elements cannot be picked yet",
        None => "click to attach",
    };
    let says = Says {
        number: None,
        name: &found.thing.label,
        hint: Some(hint),
    };
    let at = [x - origin[0], y - origin[1], w, h];
    let tokens = &hub.model.tokens;
    Some(frame(tokens, PREVIEW.to_owned(), at, &says, Look::Preview))
}

/// Where each pane of the grid is drawn in this window, by its root's key. Invariant: read on
/// every run, picking or not, so the first frame of picking already knows them.
fn panes(hub: &Hub, cx: &Cx<Win>) -> Vec<(String, &'static str, [f32; 4])> {
    let all = hub.model.grid.panes.iter();
    let keyed = all.map(|pane| (format!("{}-{}", pane.kind.slug(), pane.id.0), pane.kind));
    let laid = keyed.map(|(key, kind)| (cx.window_layout(&key), key, kind));
    let drawn: Vec<_> = laid.collect();
    let shown = drawn
        .into_iter()
        .filter_map(|(at, key, kind)| Some((key, kind.title(), at?)));
    shown.collect()
}

/// The layer that takes the pointer while picking: a crosshair over the whole window, a faint
/// box round every pane, the element under the pointer outlined, a press to attach or remove.
fn picker(
    hub: &Hub,
    cx: &Cx<Win>,
    origin: [f32; 2],
    panes: Vec<(String, &'static str, [f32; 4])>,
) -> Option<El> {
    let context = pick::context(hub).filter(|context| context.picking)?;
    let hover = context.hover.as_ref();
    let tokens = &hub.model.tokens;
    let boxed = panes.into_iter().map(|(key, name, [x, y, w, h])| {
        let says = Says {
            number: None,
            name,
            hint: None,
        };
        let at = [
            x - origin[0] + GROW,
            y - origin[1] + GROW,
            w - GROW * 2.0,
            h - GROW * 2.0,
        ];
        frame(tokens, [PANE_BOX, &key].concat(), at, &says, Look::Rest)
    });
    let layer = el()
        .key(LAYER)
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .cursor(Cursor::Crosshair)
        .focusable(true)
        .no_focus_ring()
        .on_key(|win: &mut Win, event: &KeyEvent, _: &mut Tx| {
            let leave = event.key == Key::Escape;
            if leave {
                pick::pick(win, false);
            }
            leave
        })
        .on_pointer_move_local(|_: &mut Win, at: PointerEvent, tx: &mut Tx| {
            let under = |win: &mut Win, picked, _: &mut Tx| pick::hovered(win, picked);
            tx.element_at_in(BASE, at.window_x, at.window_y, under);
        })
        .on_press(pick::pressed);
    let layer = layer.children(boxed);
    Some(layer.children(hover.and_then(|found| preview(hub, cx, found, origin))))
}

/// Draws `base` with a box over every attached thing that is on screen and, while picking,
/// the layer that takes the pointer. The boxes follow their elements and take no input.
pub fn over(base: El, hub: &Hub, cx: &mut Cx<Win>) -> El {
    let tokens = &hub.model.tokens;
    let origin = cx.window_layout(KEY).map_or([0.0; 2], |at| [at[0], at[1]]);
    let context = pick::context(hub);
    let attached = &hub.model.attached;
    let on = |address: &&Address| attached.number(address).is_some();
    let lit = context.and_then(Context::lit).filter(on);
    let mut frames = Vec::new();
    let mut drawn = Vec::new();
    for (index, thing) in attached.things.iter().enumerate() {
        let key = context.and_then(|context| context.key_of(&thing.address));
        let Some([x, y, w, h]) = key.and_then(|key| rect_of(cx, key, &thing.address)) else {
            continue;
        };
        let look = match lit {
            Some(lit) if *lit == thing.address => Look::Lit,
            Some(_) => Look::Faded,
            None => Look::Rest,
        };
        let says = Says {
            number: Some(index.saturating_add(1)),
            name: &thing.label,
            hint: None,
        };
        let at = [x - origin[0], y - origin[1], w, h];
        frames.push(frame(tokens, box_key(&thing.address), at, &says, look));
        drawn.push((thing.address.clone(), [x, y, w, h]));
    }
    let picking = context.is_some_and(|context| context.picking);
    let panes = panes(hub, cx);
    let watched = !drawn.is_empty() && !picking;
    let boxes = el()
        .key(LAYER_BOXES)
        .pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .inert()
        .children(frames);
    let whole = col()
        .key(KEY)
        .w_full()
        .h_full()
        .child(col().key(BASE).w_full().grow(1.0).min_h(0.0).child(base))
        .child(boxes)
        .children(picker(hub, cx, origin, panes));
    if watched {
        whole.on_pointer_move_local(move |win: &mut Win, at: PointerEvent, tx: &mut Tx| {
            pointed(win, &drawn, at, tx);
        })
    } else {
        whole
    }
}
