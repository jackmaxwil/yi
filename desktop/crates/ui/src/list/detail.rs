//! The detail block under an open row: facts, a text block, buttons.

use std::num::NonZeroUsize;
use std::rc::Rc;

use rune::{El, TextStyle, Tx, col, el, row, text};
use yd_core::command::table::{Run, Target, keycap};
use yd_core::ids::CommandId;
use yd_core::list::Detail;
use yd_core::list::{Main, RowHeight};
use yd_core::row::{Row, Value};

use super::row::{piece, thing};
use super::view::{Frame, Look};
use crate::kit::button::{Button, button};
use crate::menu::command_box::{self, Ran};
use crate::theme::look::{LABEL, META};
use crate::win::{Hub, Win};

/// The least room a fact of the grid needs: its label, the gap, and a few words of value.
const FACT_MIN: f32 = 132.0;
/// A value in a list of log lines is set in the mono face.
const MONO_VALUE: TextStyle = LABEL.mono();
/// The raw record of a line (record 5.5): mono, 9.5 on 14.
const RAW: TextStyle = META.lh(14.0).mono();

/// A fact's label, its value, and how wide the detail's label column is.
type Fact<'a> = (&'a str, &'a Value, f32);

/// One fact: its label, then its value. On a line of its own the value wraps; in the grid it
/// stays on one line.
fn fact(frame: &Frame, hub: &Hub, key: &str, pair: Fact<'_>, alone: bool) -> El {
    let (label, value, wide) = pair;
    let look = &frame.look;
    let font = match frame.desc.height {
        RowHeight::Row => LABEL,
        RowHeight::Line => MONO_VALUE,
    };
    let set = |said: &str| {
        let said = text(said).font(font).color(look.fg);
        let said = if alone { said.wrap() } else { said.truncate() };
        said.grow(1.0).min_w(0.0)
    };
    let key = [key, ".", label].concat();
    let label = text(label).font(META).color(look.faint).truncate().w(wide);
    let value =
        row()
            .grow(1.0)
            .min_w(0.0)
            .child(piece(&hub.model.tokens, look, &key, value, alone, set));
    let (gap, pair) = if alone {
        (8.0, row().items_start())
    } else {
        (6.0, row().center_y())
    };
    pair.gap(gap).child(label).child(value)
}

/// The key and value grid: `per_line` equal columns, filled left to right (record 5.2.1).
/// Where the list is too narrow for them the cells of a line wrap, down to one a line.
fn grid(frame: &Frame, hub: &Hub, key: &str, detail: &Detail, per_line: NonZeroUsize) -> El {
    let (facts, wide) = (&detail.facts, detail.label.0);
    let cell = |pair: Option<&(String, Value)>| match pair {
        Some((label, value)) => fact(frame, hub, key, (label, value, wide), false),
        None => el(),
    };
    let line = |line: &[(String, Value)]| {
        let cells = (0..per_line.get()).map(|at| cell(line.get(at)).grow(1.0).min_w(FACT_MIN));
        row().w_full().gaps(1.0, 12.0).flex_wrap().children(cells)
    };
    col()
        .w_full()
        .gap(1.0)
        .children(facts.chunks(per_line.get()).map(line))
}

/// The label and content grid: one fact to a line, its value free to wrap and to hold tags.
fn lines(frame: &Frame, hub: &Hub, key: &str, detail: &Detail) -> El {
    let wide = detail.label.0;
    let line = |(label, value): &(String, Value)| {
        fact(frame, hub, key, (label, value, wide), true).w_full()
    };
    col()
        .w_full()
        .gap(3.0)
        .children(detail.facts.iter().map(line))
}

/// A block of raw text, wrapped anywhere.
fn raw(look: &Look, said: &str) -> El {
    let said = text(said)
        .font(RAW)
        .color(look.dim)
        .wrap_anywhere()
        .grow(1.0)
        .min_w(0.0);
    row()
        .w_full()
        .px(7.0)
        .py(2.0)
        .radius(5.0)
        .fill(look.well)
        .child(said)
}

/// Runs a button's command as a key or a menu would. One nothing carries out says so
/// beside the buttons, until another press or another row.
fn press(frame: &Frame, win: &mut Win, tx: &mut Tx, run: Run, title: &str) {
    let said = match command_box::run(win, tx, run) {
        Ran::Done => None,
        Ran::NotBuilt(_) | Ran::Unknown => Some(format!("{title} is not available yet")),
    };
    frame.change(win, |state| state.say(said));
}

/// The buttons the table declares, then the note. Each runs its command on the row or on
/// the thing the detail aims it at, under the detail's words for it or the table's.
fn buttons(frame: &Rc<Frame>, hub: &Hub, item: &Row, key: &str, detail: &Detail) -> El {
    let look = &frame.look;
    let tokens = &hub.model.tokens;
    let table = command_box::table(hub);
    let map = command_box::keymap(hub);
    // A row whose id cannot be an address still gets its buttons, aimed at its pane.
    let aimed = thing(&frame.desc, item).map(|thing| thing.address);
    let own = aimed.map_or(Target::Pane(frame.pane), Target::Thing);
    let main = |at: usize, id: &CommandId| match detail.main {
        Main::First => at == 0,
        Main::Command(main) => main == *id,
        Main::Neither => false,
    };
    // A button no table declares would only ever say it is not available: it is not drawn.
    let known = detail
        .commands
        .iter()
        .filter(|id| table.find(**id).is_some());
    let made = known.enumerate().map(|(at, id)| {
        let declared = table.find(*id);
        let named = detail.titles.iter().find(|(command, _)| command == id);
        let title = named.map(|(_, title)| title.as_str());
        let title = title.or(declared.map(|command| command.title));
        let title = title.unwrap_or(id.0).to_owned();
        let kind = if main(at, id) {
            Button::Primary
        } else if detail.quiet.contains(id) {
            Button::Ghost
        } else {
            Button::Plain
        };
        let shown = detail.hints.iter().find(|(command, _)| command == id);
        let bound = shown.map(|(_, bound)| *bound);
        let hint = bound.or(map.key(*id)).map(keycap);
        let face = button(tokens, kind, &title, hint.as_deref()).key(format!("{key}.{}", id.0));
        let aim = detail.aims.iter().find(|(command, _)| command == id);
        let target = aim.map_or_else(|| own.clone(), |(_, thing)| Target::Thing(thing.clone()));
        let (frame, run) = (frame.clone(), Run::new(*id).on(target));
        face.on_press_tx(move |win: &mut Win, tx: &mut Tx| {
            press(&frame, win, tx, run.clone(), &title);
        })
    });
    let said = frame.said.as_deref().or(detail.note.as_deref());
    let note = said.map(|said| text(said).font(LABEL).color(look.faint).ml(4.0));
    row()
        .w_full()
        .gaps(4.0, 4.0)
        .flex_wrap()
        .center_y()
        .children(made)
        .children(note)
}

/// Everything under the open row `item`, as its list's description says.
pub(super) fn block(frame: &Rc<Frame>, hub: &Hub, item: &Row) -> El {
    let look = &frame.look;
    let detail = (frame.desc.detail)(item);
    let key = format!("{}.detail.{}", frame.key, item.id);
    let per_line = usize::from(frame.desc.facts_per_line);
    let facts = (!detail.facts.is_empty()).then(|| match NonZeroUsize::new(per_line) {
        Some(per_line) if per_line.get() > 1 => grid(frame, hub, &key, &detail, per_line),
        Some(_) | None => lines(frame, hub, &key, &detail),
    });
    let acts = (!detail.commands.is_empty() || detail.note.is_some())
        .then(|| buttons(frame, hub, item, &key, &detail));
    col()
        .key(key)
        .w_full()
        .pt(3.0)
        .pr(look.gutter)
        .pb(6.0)
        .pl(look.gutter + 12.0)
        .gap(5.0)
        .children(facts)
        .children(detail.text.as_deref().map(|said| raw(look, said)))
        .children(acts)
}
