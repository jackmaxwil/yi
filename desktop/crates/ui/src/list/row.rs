//! One row and its cells: its key and the thing it stands for, its group heading, the line
//! with its tree indent, and the detail under it when it is the open row.

use std::rc::Rc;

use rune::{
    Align, Color, Cursor, El, LineCap, Shape, TextCase, TextStyle, Tx, canvas, col, el, memo, row,
    text,
};
use yd_core::address::thing::Thing;
use yd_core::address::uri::{Address, AddressError};
use yd_core::ids::RowId;
use yd_core::list::{Col, ListDesc, RowHeight, Text, Width};
use yd_core::row::{Row, Tone, Value};
use yd_core::tokens::Tokens;

use super::view::{Frame, Look};
use super::{detail, keys};
use crate::kit::meter::meter;
use crate::kit::slider::share;
use crate::kit::sparkline::sparkline;
use crate::kit::status::{badge, dot, pill, tag};
use crate::kit::toggle::toggle;
use crate::theme::look::{LABEL, META, colour};
use crate::theme::scale::DOT_UNCOVERED;
use crate::win::{Hub, Win};

/// The bold name of the thing a row is.
pub(super) const NAME: TextStyle = TextStyle {
    weight: 600.0,
    ..LABEL
};
const NUMBER: TextStyle = LABEL.features("'tnum' 1");
const MONO: TextStyle = META.mono();
/// A line of log: the mono face on the 16 px line.
const LINE: TextStyle = LABEL.lh(16.0).mono();
/// The most tags one value and the most values one sparkline draw; data may hold any number.
const MOST: usize = 24;

/// The room between two columns (record 1.10: "8 between columns").
pub(super) const GAP: f32 = 8.0;
/// One nesting step of a tree (record 1.10: "12 for each level of nesting").
const STEP: f32 = 12.0;
/// The elbow mark before a row that has a parent: 7 by 9, reaching up towards it.
const ELBOW: (f32, f32) = (7.0, 9.0);
const GROUP: TextStyle = TextStyle {
    weight: 600.0,
    tracking: 0.57,
    ..META
};
const SUMMARY: TextStyle = TextStyle {
    weight: 500.0,
    ..META
};

/// The key of row `id` in the list keyed `list`: `<list key>.row.<row id>`.
pub fn key(list: &str, id: &RowId) -> String {
    [list, ".row.", id.as_str()].concat()
}

/// The row a row's key names, for a picker that was handed the key under the pointer.
/// Only a row's key has `.row.` in it, so no part of a pane may be named `row`.
pub fn id_of(key: &str) -> Option<RowId> {
    let (_, id) = key.split_once(".row.")?;
    (!id.is_empty()).then(|| RowId::new(id))
}

/// What a row stands for: its name, its address and the words of its cells.
pub fn thing(desc: &ListDesc, row: &Row) -> Result<Thing, AddressError> {
    let words = |column: &Col| row.words(column.field);
    let name = desc.columns.iter().find(|column| column.text == Text::Name);
    let said: Vec<&str> = desc.columns.iter().filter_map(words).collect();
    Ok(Thing {
        label: name.and_then(words).unwrap_or(row.id.as_str()).to_owned(),
        address: Address::new(desc.kind, row.id.as_str())?,
        excerpt: (!said.is_empty()).then(|| said.join(" · ")),
    })
}

/// How a column's words are set, in a table row and in a line of log.
fn style(height: RowHeight, text: Text) -> TextStyle {
    match (height, text) {
        (RowHeight::Line, Text::Name) => TextStyle {
            weight: 600.0,
            ..LINE
        },
        (RowHeight::Line, Text::Plain | Text::Mono | Text::Number | Text::Faint | Text::Mark) => {
            LINE
        }
        (RowHeight::Row, Text::Name) => NAME,
        (RowHeight::Row, Text::Plain | Text::Faint | Text::Mark) => LABEL,
        (RowHeight::Row, Text::Mono) => MONO,
        (RowHeight::Row, Text::Number) => NUMBER,
    }
}

/// A piece in its column: fixed points wide, or a share of what the fixed ones leave.
pub(super) fn fit(piece: El, width: Width) -> El {
    match width {
        Width::Px(points) => piece.w(points),
        Width::Fr(share) => piece.grow(share).min_w(0.0),
    }
}

/// One line of words in a column, cut with an ellipsis; a number stands at the right.
pub(super) fn words(words: &str, font: TextStyle, ink: Color, column: &Col) -> El {
    let words = text(words).font(font).color(ink).truncate();
    match column.text {
        Text::Number => words.align(Align::End),
        Text::Name | Text::Plain | Text::Mono | Text::Faint | Text::Mark => words,
    }
}

/// Tags in a row: on one line in a cell, wrapping in a fact. Past the most it draws, a last
/// tag says how many it left out.
fn tags(tokens: &Tokens, words: &[String], wrap: bool) -> El {
    let strip = row().w_full().min_w(0.0).gaps(3.0, 3.0).center_y();
    let strip = if wrap {
        strip.flex_wrap()
    } else {
        strip.clip_rounded(0.0)
    };
    let left_out = words.len().saturating_sub(MOST);
    strip
        .children(
            words
                .iter()
                .take(MOST)
                .map(|words| tag(tokens, words, false)),
        )
        .children((left_out > 0).then(|| tag(tokens, &format!("+{left_out}"), false)))
}

/// The stripes of a bar that waits on you: 4 px of the need colour every 8, at 135 degrees.
fn stripes(ink: Color) -> Shape {
    let step = 8.0 * std::f32::consts::SQRT_2;
    (0..64_u8).fold(Shape::new(), |shape, i| {
        let x = f32::from(i) * step;
        shape.line([x - 10.0, 10.0], [x + 2.0, -2.0], 4.0, LineCap::Butt, ink)
    })
}

/// A bar inside a time lane: where it starts and how long it is, as shares of the lane.
fn lane(look: &Look, start: f32, len: f32, tone: Tone) -> El {
    let start = share(start);
    let len = share(len).min(1.0 - start);
    let bar = el().grow(len).min_w(3.0).h(8.0).radius(2.0);
    let bar = match tone {
        Tone::Fg | Tone::Dim | Tone::Faint => bar.fill(look.bar_quiet),
        Tone::Need => bar
            .fill(look.need.alpha(0.35))
            .clip_rounded(2.0)
            .child(canvas(stripes(look.need)).w_full().h_full()),
        Tone::Accent | Tone::Ok | Tone::Bad | Tone::Warn | Tone::Model | Tone::Context => {
            bar.fill(look.tone(tone))
        }
    };
    row()
        .w_full()
        .h(8.0)
        .radius(2.0)
        .fill(look.bar_track)
        .clip_rounded(2.0)
        .child(el().grow(start))
        .child(bar)
        .child(el().grow(share(1.0 - start - len)))
}

/// How a value is drawn inside a cell or a fact, with the kit's pieces. `key` names a piece
/// that is keyed; `words` sets its text the way its place does.
pub(super) fn piece(
    tokens: &Tokens,
    look: &Look,
    key: &str,
    value: &Value,
    wrap: bool,
    words: impl Fn(&str) -> El,
) -> El {
    match value {
        Value::Text(text) => words(text),
        Value::Dot(state) => dot(tokens, *state),
        Value::Badge(text, tone) => badge(tokens, text, *tone),
        Value::Pill(text, tone) => pill(tokens, text, *tone),
        Value::Meter { ratio, text } => row()
            .center_y()
            .gap(5.0)
            .min_w(0.0)
            .child(meter(tokens, key, *ratio))
            .child(words(text)),
        Value::Tags(all) => tags(tokens, all, wrap),
        Value::Bar { start, len, tone } => lane(look, *start, *len, *tone),
        Value::Flag(on) => toggle(tokens, key, *on),
        Value::Series(values) => {
            let newest = values.get(values.len().saturating_sub(MOST)..);
            sparkline(tokens, newest.unwrap_or(values))
        }
    }
}

/// A coverage check's mark (record 5.7): 8 points, filled green when covered, a red ring for
/// a gap, a faint fill for a check that does not apply.
fn mark(tokens: &Tokens, value: &Value) -> El {
    let dot = el().size(8.0, 8.0).radius(4.0);
    match value {
        Value::Flag(true) => dot.fill(colour(tokens.ok)).opacity(0.85),
        Value::Flag(false) => dot.border(1.5, colour(tokens.bad)),
        _ => dot.fill(colour(DOT_UNCOVERED.of(tokens))),
    }
}

/// One cell. `toned` is the colour of the row's role, which its plain words take.
fn cell(
    frame: &Frame,
    tokens: &Tokens,
    key: &str,
    column: &Col,
    width: Width,
    value: Option<&Value>,
    toned: Option<Color>,
) -> El {
    let look = &frame.look;
    let ink = match column.text {
        Text::Name => look.fg,
        Text::Faint => look.faint,
        Text::Plain | Text::Mono | Text::Number | Text::Mark => toned.unwrap_or(look.dim),
    };
    let font = style(frame.desc.height, column.text);
    let set = |said: &str| words(said, font, ink, column);
    match value {
        None => fit(el(), width),
        Some(value) if column.text == Text::Mark => {
            let drawn = mark(tokens, value).key([key, ".", column.field].concat());
            fit(row().center_y().child(drawn), width)
        }
        Some(Value::Text(said)) => fit(set(said), width),
        Some(Value::Badge(said, tone)) => fit(badge(tokens, said, *tone), width),
        Some(drawn) => {
            let key = [key, ".", column.field].concat();
            let drawn = piece(tokens, look, &key, drawn, false, set);
            fit(row().center_y().child(drawn), width)
        }
    }
}

/// How far the indent and elbow of a row `depth` levels down reach: nothing for a root.
fn reach(depth: u8) -> Option<f32> {
    (depth > 0).then(|| f32::from(depth) * STEP + ELBOW.0)
}

/// The indent and elbow before the first column of a row that has a parent.
fn lead(look: &Look, reach: f32, height: f32) -> El {
    let elbow = el()
        .absolute(reach - ELBOW.0, 2.0)
        .size(ELBOW.0, ELBOW.1)
        .border_sides([0.0, 0.0, 1.0, 1.0], look.tree)
        .corners([0.0, 0.0, 0.0, 4.0]);
    el().w(reach).h(height).child(elbow)
}

/// The row itself: its cells in their columns, pressed to open and close it.
fn line(frame: &Rc<Frame>, tokens: &Tokens, item: &Row, depth: u8, slot: usize) -> El {
    let (look, desc) = (&frame.look, &frame.desc);
    let height = frame.row_height();
    let reach = reach(depth);
    // Invariant: the indent comes out of the name's column, so the columns after it stay in line.
    let named = desc
        .columns
        .iter()
        .position(|column| column.text == Text::Name);
    let toned = item.tone.map(|tone| look.tone(tone));
    let row_key = key(&frame.key, &item.id);
    let cells = super::view::shown(desc, frame.kept).map(|(at, column)| {
        let width = match (column.width, reach) {
            (Width::Px(points), Some(reach)) if Some(at) == named => {
                Width::Px((points - reach - GAP).max(0.0))
            }
            (width, _) => width,
        };
        cell(
            frame,
            tokens,
            &row_key,
            column,
            width,
            item.get(column.field),
            toned,
        )
    });
    let (frame, id) = (frame.clone(), item.id.clone());
    let line = row()
        .w_full()
        .h(height)
        .px(look.gutter)
        .gap(GAP)
        .center_y()
        .children(reach.map(|reach| lead(look, reach, height)))
        .children(cells)
        .cursor(Cursor::Pointer)
        // Invariant: a row kept by memo keeps this handler and its frame: the handler reads
        // the list's state when it runs, and `slot` is only a hint, checked by id.
        .on_background_press_tx(move |win: &mut Win, tx: &mut Tx| {
            keys::press(&frame, win, &id, slot, tx);
        });
    // Record 5.5: an error line carries its tint.
    let line = if item.tone == Some(Tone::Bad) {
        line.fill(look.bad.alpha(0.13))
    } else {
        line
    };
    if item.dim { line.opacity(0.5) } else { line }
}

/// The heading over the first row of a group: its name, then the summary that row carries
/// in its `group.summary` cell.
fn heading(frame: &Frame, item: &Row, group: &str) -> El {
    let look = &frame.look;
    let name = (!group.is_empty()).then(|| {
        text(group)
            .font(GROUP)
            .text_case(TextCase::Uppercase)
            .color(look.dim)
    });
    let summary = item
        .words("group.summary")
        .map(|words| text(words).font(SUMMARY).color(look.faint).truncate());
    row()
        .key(format!("{}.group.{}", frame.key, item.id))
        .w_full()
        .h(look.row)
        .mt(8.0)
        .px(look.gutter)
        .gap(6.0)
        .center_y()
        .children(name)
        .children(summary)
}

/// The 2 px bar down the left edge of the selected row and of the open row and its detail.
fn bar(frame: &Frame, cursor: bool) -> El {
    let bar = el()
        .pin(Some(0.0), Some(0.0), None, Some(0.0))
        .w(2.0)
        .fill(frame.look.picked);
    if cursor {
        bar.key(format!("{}.cursor", frame.key))
    } else {
        bar
    }
}

/// The row at `slot`: a heading if it starts a group, its line, and its detail if it is open.
pub(super) fn build(frame: &Rc<Frame>, hub: &Hub, slot: usize) -> Option<El> {
    let (item, depth) = frame.at(hub, slot)?;
    let before = slot.checked_sub(1).and_then(|slot| frame.at(hub, slot));
    let starts = before.is_none_or(|(before, _)| before.group != item.group);
    let (open, cursor) = (frame.is_open(&item.id), frame.is_cursor(&item.id));
    let make = || {
        let group = item.group.as_deref().filter(|_| starts);
        let block = col()
            .w_full()
            .child(line(frame, &hub.model.tokens, item, depth, slot))
            .children(open.then(|| detail::block(frame, hub, item)))
            .children((open || cursor).then(|| bar(frame, cursor)));
        let block = if open {
            block.fill(frame.look.selected)
        } else {
            block
        };
        // The keyboard's place in a list is its cursor row: the ring goes round that row,
        // not round the whole list (Rune draws a focused control's ring on the child set).
        let block = if cursor {
            block.focus_ring(crate::a11y::RING, -crate::a11y::RING, frame.look.ring)
        } else {
            block
        };
        col()
            .w_full()
            .children(group.map(|group| heading(frame, item, group)))
            .child(block)
    };
    Some(memo(
        key(&frame.key, &item.id),
        (
            frame.stamp(),
            open,
            cursor,
            open.then(|| frame.said.clone()),
        ),
        make,
    ))
}
