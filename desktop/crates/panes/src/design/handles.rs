//! The design box over the web page: the context box, eight handles, the size badge and two
//! guides. Also what design mode does with the page's input, and the element as a thing.

use rune::{Cursor, Cx, Drag, El, Phase, TextStyle, Tx, el, row, text};
use yd_core::address::thing::Thing;
use yd_core::command::tools::design::{AT, MAX_HTML, MAX_SIDE, Property, SET, pair};
use yd_core::ids::PaneId;
use yd_core::row::Row;
use yd_core::tokens::{Rgba, Tokens};
use yd_ui::overlay::boxes::numbered;
use yd_ui::pane::PaneCx;
use yd_ui::screen::input::{Input, scale};
use yd_ui::screen::surface::Screen;
use yd_ui::theme::look::{CODE, colour};
use yd_ui::theme::scale::{GUIDE, HANDLE, ON_CONTEXT, ON_MODEL};
use yd_ui::win::{Hub, Win};

use super::changes;
use super::pane::{ask, element, mode, set};

/// What the picked element's key ends with: `<web pane>.design.element`.
const ELEMENT_KEY: &str = ".design.element";
/// The context box is its element grown by this much; the handles sit on its edge.
const GROW: f32 = 3.0;
const HANDLE_SIDE: f32 = 7.0;
/// A distance of this many pixels or fewer gets no guide.
const NO_GUIDE: u32 = 2;
const BADGE_TALL: f32 = 15.0;
const BADGE: TextStyle = TextStyle::new(9.5, 600.0).lh(BADGE_TALL).family(CODE);
const MEASURE_TALL: f32 = 13.0;
const MEASURE: TextStyle = TextStyle::new(9.0, 600.0).lh(MEASURE_TALL).family(CODE);

/// The eight handles: a name, where on the box it sits, and which way a drag grows the box.
const HANDLES: [(&str, f32, f32); 8] = [
    ("nw", 0.0, 0.0),
    ("n", 0.5, 0.0),
    ("ne", 1.0, 0.0),
    ("e", 1.0, 0.5),
    ("se", 1.0, 1.0),
    ("s", 0.5, 1.0),
    ("sw", 0.0, 1.0),
    ("w", 0.0, 0.5),
];

/// Where the element is over the surface, in points, and how many points one CSS pixel is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub rect: [f32; 4],
    pub per: f32,
}

/// The element of `row` over a picture of `picture` pixels fitted to a surface of `surface`
/// points. None before the first picture, and for a row whose numbers do not read.
pub fn fit(row: &Row, picture: (u32, u32), surface: [f32; 2]) -> Option<Fit> {
    let points = scale(picture, surface[0], surface[1])?;
    let number = |cell: &str| {
        row.words(cell)?
            .parse::<f32>()
            .ok()
            .filter(|n| n.is_finite())
    };
    let at = |cell: &str| Some(number(cell)? * points);
    let ratio = number("ratio").filter(|ratio| *ratio > 0.0)?;
    Some(Fit {
        rect: [at("x")?, at("y")?, at("w")?, at("h")?],
        per: points * ratio,
    })
}

/// A distance the page measured, when it is far enough to be worth a guide.
pub fn measured(row: &Row, cell: &str) -> Option<u32> {
    let far = row.words(cell)?.parse::<u32>().ok()?;
    (far > NO_GUIDE).then_some(far)
}

/// The size a drag of `(dx, dy)` points on a handle gives an element of `from` CSS pixels.
pub fn resized(from: (u32, u32), (dx, dy): (f32, f32), handle: (f32, f32), per: f32) -> (u32, u32) {
    let grown = |from: u32, by: f32, sits: f32| {
        let way = (sits - 0.5) * 2.0;
        let to = from as f32 + by * way / per.max(0.01);
        to.round().clamp(1.0, MAX_SIDE as f32) as u32
    };
    (grown(from.0, dx, handle.0), grown(from.1, dy, handle.1))
}

/// A handle's drag: the size at the press is kept, each move asks the page for the size the
/// pointer has reached, and only when it differs from the one last asked.
fn dragged(win: &mut Win, drag: &Drag, handle: (f32, f32), per: f32) {
    let read = |win: &Win| {
        let hub = win.hub.try_borrow().ok()?;
        let size = element(&hub.model)?
            .words(Property::Size.slug())
            .and_then(pair);
        let held = mode(&hub).map(|mode| (mode.grip, mode.asked));
        Some((size, held?))
    };
    let Some((size, (grip, asked))) = read(win) else {
        return;
    };
    match drag.phase {
        Phase::Down => set(win, |mode| (mode.grip, mode.asked) = (size, size)),
        Phase::Move => {
            let Some(from) = grip else { return };
            let to = resized(from, (drag.dx, drag.dy), handle, per);
            if asked != Some(to) {
                set(win, |mode| mode.asked = Some(to));
                ask(win, SET, Some(format!("size={} × {}", to.0, to.1)));
            }
        }
        Phase::Up => set(win, |mode| (mode.grip, mode.asked) = (None, None)),
    }
}

/// How wide a tag of `words` is with `pad` points each side.
fn tag_width(words: &str, font: TextStyle, pad: f32) -> f32 {
    rune::text::advance(words, font) + pad * 2.0
}

/// A tag of measured numbers at `at`: the badge under the box, a guide's distance.
fn tag(key: String, words: &str, font: TextStyle, ink: Rgba, at: [f32; 4]) -> El {
    row()
        .key(key)
        .absolute(at[0], at[1])
        .size(at[2], at[3])
        .center()
        .child(text(words).font(font).color(colour(ink)))
}

/// One guide: a one-point line from `from`, `len` points down or across, its distance on a
/// tag centred on it.
fn guide(
    tokens: &Tokens,
    key: String,
    from: (f32, f32),
    len: f32,
    down: bool,
    far: u32,
) -> [El; 2] {
    let pink = colour(GUIDE.of(tokens));
    let (w, h) = if down { (1.0, len) } else { (len, 1.0) };
    let line = el().absolute(from.0, from.1).size(w, h).fill(pink);
    let words = far.to_string();
    let wide = tag_width(&words, MEASURE, 4.0);
    let (left, top) = (from.0 + (w - wide) / 2.0, from.1 + (h - MEASURE_TALL) / 2.0);
    // A short guide at the page's edge would centre its tag half off the page.
    let at = [left.max(0.0), top, wide, MEASURE_TALL];
    let label = tag(key, &words, MEASURE, ON_MODEL.of(tokens), at);
    [line, label.radius(3.0).fill(pink)]
}

/// The design box over the page of the web pane keyed `key`: none while design mode is off,
/// before an element is picked, or before the page has a picture.
pub fn layer(pane: &PaneCx<'_>, cx: &mut Cx<Win>, key: &str, screen: &Screen) -> Option<El> {
    // Invariant: the read is made on every run, found or not, or it answers a run late.
    let surface = cx.layout_size(&format!("{key}.page"));
    let tokens = pane.tokens();
    mode(pane.hub).filter(|mode| mode.on)?;
    let row = element(pane.model())?;
    let Fit { rect, per } = fit(row, screen.size()?, surface?)?;
    let word = |cell: &str| row.words(cell).unwrap_or_default();
    let attached = &pane.model().attached;
    let own = changes::element(word("page"), word("name"));
    let worn = own.as_ref().and_then(|own| attached.number(own));
    let number = worn.unwrap_or(attached.things.len().saturating_add(1));
    let frame = numbered(
        tokens,
        format!("{key}{ELEMENT_KEY}"),
        rect,
        number,
        word("name"),
    );
    let (x, y, w, h) = (
        rect[0] - GROW,
        rect[1] - GROW,
        rect[2] + GROW * 2.0,
        rect[3] + GROW * 2.0,
    );
    let handles = HANDLES.iter().map(|(name, fx, fy)| {
        let (fx, fy) = (*fx, *fy);
        let cursor = match (fx == 0.5, fy == 0.5) {
            (true, _) => Cursor::ResizeRow,
            (_, true) => Cursor::ResizeColumn,
            (false, false) => Cursor::Move,
        };
        el().key(format!("{key}.design.handle.{name}"))
            .absolute(
                x + w * fx - HANDLE_SIDE / 2.0,
                y + h * fy - HANDLE_SIDE / 2.0,
            )
            .size(HANDLE_SIDE, HANDLE_SIDE)
            .radius(1.5)
            .fill(colour(HANDLE.of(tokens)))
            .border(1.5, colour(tokens.context))
            .cursor(cursor)
            .on_drag(move |win: &mut Win, drag: &mut Drag, _: &mut Tx| {
                dragged(win, drag, (fx, fy), per);
            })
    });
    let size = word(Property::Size.slug());
    let wide = tag_width(size, BADGE, 5.0);
    let under = [x + w - wide, y + h + 7.0, wide, BADGE_TALL];
    let on = ON_CONTEXT.of(tokens);
    let badge = tag(format!("{key}.design.size"), size, BADGE, on, under);
    let badge = badge.radius(4.0).fill(colour(tokens.context));
    let below = measured(row, "gap").map(|far| {
        let from = (rect[0] + rect[2] * 0.3, rect[1] + rect[3]);
        guide(
            tokens,
            format!("{key}.design.gap"),
            from,
            far as f32 * per,
            true,
            far,
        )
    });
    let beside = measured(row, "inset").map(|far| {
        let len = far as f32 * per;
        let from = (rect[0] - len, rect[1] + rect[3] * 0.74);
        guide(tokens, format!("{key}.design.inset"), from, len, false, far)
    });
    let guides = below.into_iter().chain(beside).flatten();
    Some(
        el().key(format!("{key}.design"))
            .absolute(0.0, 0.0)
            .w_full()
            .h_full()
            .child(frame)
            .children(handles)
            .child(badge)
            // Incident: a short inset's tag sits on the box's own edge, and the frame and a
            // handle drawn after it cut its number in two. The guides are drawn last.
            .children(guides),
    )
}

/// What design mode does with the page's input: off, it is the page's; on, a press picks
/// when a pick is wanted, the size passes, and the rest is held back (scroll in Browse).
pub fn took(win: &mut Win, input: Input) -> Option<Input> {
    let (on, picking) = {
        let hub = win.hub.try_borrow().ok()?;
        mode(&hub).map_or((false, false), |mode| (mode.on, mode.picking))
    };
    if !on {
        return Some(input);
    }
    match input {
        Input::Resized { .. } => Some(input),
        Input::Press(at) if picking => {
            set(win, |mode| mode.picking = false);
            ask(win, AT, Some(format!("{},{}", at.x, at.y)));
            None
        }
        Input::Press(_)
        | Input::Move(_)
        | Input::Release(_)
        | Input::Wheel { .. }
        | Input::Text(_)
        | Input::Key(_) => None,
    }
}

/// What a picked element says and its HTML, with a notice where the HTML was cut.
fn words(row: &Row) -> String {
    let word = |cell: &str| row.words(cell).unwrap_or_default();
    let html = word("html");
    let kept = html.chars().count();
    let size = word("html_size").parse::<usize>().unwrap_or(kept);
    let mut lines = vec![word(Property::Says.slug()).to_owned()];
    if !html.is_empty() {
        lines.push(format!("HTML: {html}"));
    }
    if size > kept {
        lines.push(format!(
            "[…] kept {kept} of {size} characters of its HTML (cap {MAX_HTML})"
        ));
    }
    lines.retain(|line| !line.is_empty());
    lines.join("\n")
}

/// The web pane's finder: the design box stands for the picked element, by its address
/// `yi://element/<page>#<name>`, with what it says and its HTML as its excerpt.
pub fn thing_at(hub: &Hub, _: PaneId, key: &str) -> Option<Thing> {
    key.strip_suffix(ELEMENT_KEY)?;
    let row = element(&hub.model)?;
    let word = |cell: &str| row.words(cell).unwrap_or_default();
    Some(Thing {
        label: word("name").to_owned(),
        address: changes::element(word("page"), word("name"))?,
        excerpt: Some(words(row)).filter(|said| !said.is_empty()),
    })
}
