//! Mark-up on a screen: a box round a region with a note beside it. Nothing on the far side
//! is changed; the note and the region's address go with the prompt like any attached thing.

use rune::{Cx, El, Shadow, TextEdit, TextStyle, Tx, col, el, text, text_input};
use yd_core::address::attach::AttachMsg;
use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::ids::PaneId;
use yd_core::kind::Kind;
use yd_core::msg::Msg;
use yd_ui::overlay::boxes::numbered;
use yd_ui::pane::PaneCx;
use yd_ui::screen::input::{Input, Point, scale};
use yd_ui::screen::surface::Screen;
use yd_ui::theme::look::colour;
use yd_ui::theme::scale::{MEDIUM, ON_NOTE, SHADOW_NEAR};
use yd_ui::win::{Win, dispatch, field};

/// The field a note is typed into, in the window that edits it.
const NOTE: &str = "note";
/// Notes one screen holds: as many as a prompt carries things.
pub const MAX_NOTES: usize = 12;
/// The most characters of one note.
pub const MAX_WORDS: usize = 280;
/// A region narrower or shorter than this many pixels is a slip of the hand, not a region.
const MIN_SIDE: u32 = 4;
/// A note's bubble: 136 wide, 9 right of its box and 3 above the box's top (record 4.9.2).
const BUBBLE: (f32, f32, f32) = (136.0, 9.0, 3.0);
const WORDS: TextStyle = TextStyle::new(10.5, MEDIUM).lh(14.2);

/// A region of the picture, in its pixels, and what is said about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub at: Point,
    pub width: u32,
    pub height: u32,
    /// What the region is called on its chip; never parsed.
    pub label: String,
    pub words: String,
}

impl Note {
    /// The region as a thing: `yi://screen/<screen>#<x>,<y>,<w>,<h>` in the picture's pixels,
    /// the note as its excerpt.
    pub fn thing(&self, screen: &str) -> Option<Thing> {
        let (at, w, h) = (self.at, self.width, self.height);
        let region = format!("{},{},{w},{h}", at.x, at.y);
        Some(Thing {
            label: self.label.clone(),
            address: Address::new(Kind::Screen, screen).ok()?.part(region),
            excerpt: Some(self.words.clone()).filter(|words| !words.is_empty()),
        })
    }
}

/// One screen pane's mark-up: its notes, the region being dragged out, the note being typed.
#[derive(Debug, Default)]
pub struct Marks {
    pub notes: Vec<Note>,
    dragging: Option<(Point, Point)>,
    writing: Option<usize>,
}

fn change<T>(win: &mut Win, pane: PaneId, edit: impl FnOnce(&mut Marks) -> T) -> Option<T> {
    let mut hub = win.hub.try_borrow_mut().ok()?;
    hub.panes.get_mut::<Marks>(Some(pane)).map(edit)
}

/// The region two corners span, when it is big enough to be one.
fn region(a: Point, b: Point, screen: &str) -> Option<Note> {
    let (left, top) = (a.x.min(b.x), a.y.min(b.y));
    let (width, height) = (a.x.abs_diff(b.x), a.y.abs_diff(b.y));
    (width >= MIN_SIDE && height >= MIN_SIDE).then(|| Note {
        at: Point { x: left, y: top },
        width,
        height,
        label: format!("{screen} · {width} × {height} at {left}, {top}"),
        words: String::new(),
    })
}

/// A screen's input while it is marked up: a drag draws a region, whose note is then typed
/// in its bubble. True for everything, since nothing of mark-up reaches the far side.
pub fn took(win: &mut Win, pane: PaneId, screen: &str, input: Input) -> bool {
    let sized = matches!(input, Input::Resized { .. });
    change(win, pane, |marks| match input {
        Input::Press(at) if marks.notes.len() < MAX_NOTES => marks.dragging = Some((at, at)),
        Input::Move(at) => marks.dragging = marks.dragging.map(|(from, _)| (from, at)),
        Input::Release(at) => {
            let from = marks.dragging.take().map(|(from, _)| from);
            if let Some(note) = from.and_then(|from| region(from, at, screen)) {
                marks.writing = Some(marks.notes.len());
                marks.notes.push(note);
            }
        }
        Input::Press(_)
        | Input::Wheel { .. }
        | Input::Text(_)
        | Input::Key(_)
        | Input::Resized { .. } => {}
    });
    !sized
}

/// Keeps what was typed as the note's words and attaches the region; an empty note drops it.
fn written(win: &mut Win, pane: PaneId, screen: &str) {
    let typed = win.edits.remove(&field(pane, NOTE));
    let typed = typed.map(|edit| edit.text.clone()).unwrap_or_default();
    let words: String = typed.trim().chars().take(MAX_WORDS).collect();
    let thing = change(win, pane, |marks| {
        let at = marks.writing.take()?;
        if words.is_empty() {
            (at < marks.notes.len()).then(|| marks.notes.remove(at));
            return None;
        }
        let note = marks.notes.get_mut(at)?;
        note.words = words;
        note.thing(screen)
    });
    if let Some(thing) = thing.flatten() {
        dispatch(win, Msg::Attach(AttachMsg::Toggle(thing)));
    }
}

/// The note beside its box: its words, or the field while it is being typed.
fn bubble(pane: &PaneCx<'_>, key: String, note: &Note, typing: bool, screen: &str) -> El {
    let tokens = pane.tokens();
    let ink = colour(ON_NOTE.of(tokens));
    let id = pane.pane;
    let said = if typing {
        let blank = TextEdit::new("");
        let edit = pane.edits.and_then(|edits| edits.get(&field(id, NOTE)));
        let screen = screen.to_owned();
        text_input(edit.unwrap_or(&blank), WORDS)
            .key(format!("{key}.field"))
            .w_full()
            .color(ink)
            .placeholder(
                "Type a note, then Return",
                colour(ON_NOTE.of(tokens).alpha(0.6)),
            )
            .label("Note")
            .caret(ink, colour(ON_NOTE.of(tokens).alpha(0.3)))
            .on_edit(move |win: &mut Win| win.edits.entry(field(id, NOTE)).or_default())
            .on_submit(move |win: &mut Win, _: &mut Tx| written(win, id, &screen))
    } else {
        text(note.words.as_str()).font(WORDS).color(ink).wrap()
    };
    col()
        .key(key)
        .w(BUBBLE.0)
        .px(7.0)
        .py(5.0)
        .corners([2.0, 8.0, 8.0, 8.0])
        .fill(colour(tokens.need))
        .box_shadow(Shadow {
            y: 2.0,
            blur: 6.0,
            color: colour(SHADOW_NEAR.of(tokens)),
            ..Shadow::default()
        })
        .child(said)
}

/// The mark-up over the surface keyed `key` showing `screen`, the far side named `named`:
/// a numbered box and a note for every region that is being written or is still attached.
pub fn layer(pane: &PaneCx<'_>, cx: &mut Cx<Win>, key: &str, screen: &Screen, named: &str) -> El {
    // Invariant: the read is made on every run, found or not, or it answers a run late.
    let surface = cx.layout_size(key);
    let layer = el()
        .key(format!("{key}.marks"))
        .absolute(0.0, 0.0)
        .w_full()
        .h_full();
    let fitted = screen.size().zip(surface);
    let Some(points) = fitted.and_then(|(picture, box_)| scale(picture, box_[0], box_[1])) else {
        return layer;
    };
    let tokens = pane.tokens();
    let fallback = Marks::default();
    let marks = pane.state::<Marks>().unwrap_or(&fallback);
    let attached = &pane.model().attached;
    let rect = |at: Point, w: u32, h: u32| {
        let pt = |v: u32| v as f32 * points;
        [pt(at.x), pt(at.y), pt(w), pt(h)]
    };
    let mut drawn = Vec::new();
    let mut next = attached.things.len();
    for (at, note) in marks.notes.iter().enumerate() {
        let typing = marks.writing == Some(at);
        let worn = note
            .thing(named)
            .and_then(|thing| attached.number(&thing.address));
        let number = match worn {
            Some(number) => number,
            None if typing => {
                next = next.saturating_add(1);
                next
            }
            None => continue,
        };
        let r = rect(note.at, note.width, note.height);
        let own = format!("{key}.mark.{at}");
        let (left, top) = (r[0] + r[2] + 3.0 + BUBBLE.1, r[1] - 3.0 - BUBBLE.2);
        let note = bubble(pane, format!("{own}.note"), note, typing, named);
        drawn.push(numbered(tokens, own, r, number, ""));
        drawn.push(note.absolute(left, top));
    }
    let dragged = marks.dragging.and_then(|(a, b)| region(a, b, named));
    let dragged = dragged.map(|note| {
        let r = rect(note.at, note.width, note.height);
        numbered(
            tokens,
            format!("{key}.mark.new"),
            r,
            next.saturating_add(1),
            "",
        )
    });
    layer.children(drawn).children(dragged)
}
