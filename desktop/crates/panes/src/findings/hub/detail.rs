//! An opened finding in the hub: its reason, evidence, proposed fix, proof, history and the
//! actions it offers (record 6.6.2), drawn under its row.

use rune::{Cursor, El, col, el, row, spacer, text};
use yd_core::command::table::{Run, Target};
use yd_core::kind::Kind;
use yd_core::row::{Row, Value};
use yd_core::tokens::{Rgba, Tokens};
use yd_ui::kit::button::{Button, button};
use yd_ui::kit::label::section;
use yd_ui::kit::status::{badge, pill, tag};
use yd_ui::pane::PaneCx;
use yd_ui::theme::look::{CODE_LINE, LABEL, META, colour};
use yd_ui::theme::scale::{DETAIL, FIX, LINE_CARD};
use yd_ui::win::Win;

use super::press;
use crate::findings::desc::{SEND_TO_TRACKER, buttons, mark_of};
use crate::findings::pane::Item;
use yd_core::command::by_kind::FINDING_DISMISS;
use yd_core::findings::state::Mark;

const HEADLINE: rune::TextStyle = rune::TextStyle::new(12.5, 650.0).lh(15.6);
const PROSE: rune::TextStyle = LABEL.lh(14.7);

fn tags<'a>(row: &'a Row, field: &str) -> &'a [String] {
    match row.get(field) {
        Some(Value::Tags(said)) => said,
        _ => &[],
    }
}

/// `both pass`, `it passes`, `all 3 pass`: how many checks must pass before it closes.
fn closes(n: usize) -> String {
    match n {
        1 => "closes itself when it passes".to_owned(),
        2 => "closes itself when both pass".to_owned(),
        n => format!("closes itself when all {n} pass"),
    }
}

/// A check's 9 px mark: filled when met or failing, a ring while waiting or pending.
pub fn mark(tokens: &Tokens, mark: Mark) -> El {
    let disc = el().size(9.0, 9.0).radius(4.5);
    match mark {
        Mark::Met => disc.fill(colour(tokens.ok)),
        Mark::Failing => disc.fill(colour(tokens.bad)),
        Mark::Waiting => disc.border(1.5, colour(tokens.need)),
        Mark::Pending => disc.border(1.5, colour(tokens.faint)),
    }
}

/// One check: its mark, what must hold, and where it stands.
pub fn check(tokens: &Tokens, key: String, said: &str, mark_at: Mark, note: &str) -> El {
    row()
        .key(key)
        .w_full()
        .h(20.0)
        .gap(7.0)
        .center_y()
        .child(mark(tokens, mark_at))
        .child(
            text(said)
                .font(LABEL)
                .color(colour(tokens.fg))
                .truncate()
                .grow(1.0)
                .min_w(0.0),
        )
        .child(text(note).font(META).color(colour(tokens.faint)))
}

/// The checks a row carries, else its one proof statement.
fn proof(tokens: &Tokens, key: &str, row: &Row) -> Vec<El> {
    let said = tags(row, "checks");
    let notes = tags(row, "check_notes");
    let marks = tags(row, "check_marks");
    if said.is_empty() {
        let Some(statement) = row.words("proof") else {
            return Vec::new();
        };
        let state = row.words("proof_state").unwrap_or_default();
        let at = if state == "not started" {
            Mark::Pending
        } else {
            Mark::Waiting
        };
        return vec![check(
            tokens,
            format!("{key}.check.0"),
            statement,
            at,
            state,
        )];
    }
    said.iter()
        .enumerate()
        .map(|(n, words)| {
            let note = notes.get(n).map_or("", String::as_str);
            let at = marks.get(n).map_or(Mark::Pending, |mark| mark_of(mark));
            check(tokens, format!("{key}.check.{n}"), words, at, note)
        })
        .collect()
}

/// An evidence chip: its number, its label; a press shows the file it points at.
fn evidence(pane: &PaneCx<'_>, key: &str, item: &Item, n: usize, label: &str) -> El {
    let tokens = pane.tokens();
    let ink = tokens.context;
    let number = row()
        .w(14.0)
        .h(14.0)
        .center()
        .radius(4.0)
        .fill(colour(ink))
        .child(
            text((n + 1).to_string())
                .font(META)
                .color(colour(tokens.window)),
        );
    let chip = row()
        .key(format!("{key}.evidence.{}.{n}", item.row.id.as_str()))
        .h(18.0)
        .pl(2.0)
        .pr(6.0)
        .gap(5.0)
        .center_y()
        .radius(5.0)
        .fill(colour(ink.alpha(if n == 0 { 0.22 } else { 0.1 })))
        .border(1.0, colour(ink.alpha(if n == 0 { 0.5 } else { 0.18 })))
        .child(number)
        .child(text(label).font(META).color(colour(tokens.fg)));
    let Some(thing) = item.finding.evidence.get(n) else {
        return chip;
    };
    let address = thing.address.clone();
    if !matches!(address.kind(), Kind::File | Kind::Lines) {
        return chip;
    }
    chip.cursor(Cursor::Pointer)
        .on_press_tx(move |win: &mut Win, tx: &mut rune::Tx| {
            yd_ui::grid_view::view::open_file(win, tx, address.clone());
        })
}

/// A line of the proposed fix, tinted as removed or added.
fn fix_line(tokens: &Tokens, words: &str) -> El {
    let tint: Rgba = match words.chars().next() {
        Some('-') => tokens.removed.alpha(0.12),
        Some('+') => tokens.added.alpha(0.12),
        _ => tokens.removed.alpha(0.0),
    };
    row().w_full().px(7.0).fill(colour(tint)).child(
        text(words)
            .font(CODE_LINE)
            .color(colour(tokens.dim))
            .truncate(),
    )
}

fn history(tokens: &Tokens, said: &[String]) -> El {
    let pair = |pair: &[String]| {
        let when = pair.first().map_or("", String::as_str);
        let what = pair.get(1).map_or("", String::as_str);
        row()
            .gap(6.0)
            .child(
                text(when)
                    .font(META.mono())
                    .color(colour(tokens.faint))
                    .w(34.0),
            )
            .child(
                text(what)
                    .font(META)
                    .color(colour(tokens.dim))
                    .wrap()
                    .grow(1.0)
                    .min_w(0.0),
            )
    };
    col().w_full().gap(1.0).children(said.chunks(2).map(pair))
}

/// The action bar: the row's buttons, the first filled, Dismiss quiet, the tracker last.
fn actions(pane: &PaneCx<'_>, key: &str, item: &Item, said: Option<&str>) -> El {
    let tokens = pane.tokens();
    let aim = item.finding.address().map_or(Target::App, Target::Thing);
    let mut left = Vec::new();
    let mut right = Vec::new();
    for (at, (command, words, keyed)) in buttons(&item.row).into_iter().enumerate() {
        let kind = match (at, command) {
            (0, _) => Button::Primary,
            (_, FINDING_DISMISS | SEND_TO_TRACKER) => Button::Ghost,
            _ => Button::Plain,
        };
        let hint = keyed.then_some("↵");
        let run = Run::new(command).on(aim.clone());
        let face = button(tokens, kind, &words, hint)
            .key(format!("{key}.{}", command.0))
            .on_press_tx(press(pane.pane, run, words.clone()));
        if command == SEND_TO_TRACKER {
            right.push(face);
        } else {
            left.push(face);
        }
    }
    let note = said.map(|said| text(said).font(META).color(colour(tokens.faint)));
    row()
        .w_full()
        .h(30.0)
        .px(1.0)
        .gap(6.0)
        .center_y()
        .border_sides([1.0, 0.0, 0.0, 0.0], colour(LINE_CARD.of(tokens)))
        .children(left)
        .children(note)
        .child(spacer())
        .children(right)
}

/// The opened finding under its row.
pub fn view(pane: &PaneCx<'_>, key: &str, item: &Item, said: Option<&str>) -> El {
    let tokens = pane.tokens();
    let item_row = &item.row;
    let key = format!("{key}.detail.{}", item_row.id.as_str());
    let mut top = Vec::new();
    if let Some(Value::Badge(words, level)) = item_row.get("severity") {
        top.push(badge(tokens, words, *level));
    }
    if let Some(Value::Pill(words, role)) = item_row.get("state") {
        top.push(pill(tokens, words, *role));
    }
    if item_row.words("blocks").is_some() {
        top.push(tag(tokens, "blocks calling this thread done", true));
    }
    top.push(spacer());
    if let Some(name) = item_row.words("ref") {
        top.push(text(name).font(META).color(colour(tokens.faint)));
    }
    let why = item_row
        .words("explanation")
        .or_else(|| item_row.words("why"));
    let proved = proof(tokens, &key, item_row);
    let shown = tags(item_row, "evidence");
    let fix = tags(item_row, "fix");
    let said_before = tags(item_row, "history");
    let places = match shown.len() {
        1 => "1 place".to_owned(),
        n => format!("{n} places"),
    };
    let chips = shown
        .iter()
        .enumerate()
        .map(|(n, label)| evidence(pane, &key, item, n, label));
    let mut body = col()
        .key(key.clone())
        .w_full()
        .px(8.0)
        .pt(8.0)
        .gap(8.0)
        .fill(colour(DETAIL.of(tokens)))
        .child(row().w_full().gap(6.0).center_y().children(top))
        .child(
            text(item_row.words("finding").unwrap_or_default())
                .font(HEADLINE)
                .color(colour(tokens.fg))
                .wrap(),
        )
        .children(why.map(|why| text(why).font(PROSE).color(colour(tokens.dim)).wrap()));
    if !shown.is_empty() {
        body = body
            .child(section(tokens, "Evidence", None, Some(&places)).px(0.0))
            .child(row().w_full().gap(4.0).flex_wrap().children(chips));
    }
    if !fix.is_empty() {
        let by = item_row.words("fix_note");
        body = body
            .child(section(tokens, "Proposed fix", None, by).px(0.0))
            .child(
                col()
                    .w_full()
                    .py(2.0)
                    .radius(5.0)
                    .fill(colour(FIX.of(tokens)))
                    .children(fix.iter().map(|line| fix_line(tokens, line))),
            );
    }
    if !proved.is_empty() {
        let note = closes(proved.len());
        body = body
            .child(section(tokens, "Proof", None, Some(&note)).px(0.0))
            .child(col().w_full().children(proved));
    }
    if !said_before.is_empty() {
        body = body
            .child(section(tokens, "History", None, None).px(0.0))
            .child(history(tokens, said_before));
    }
    body.child(actions(pane, &key, item, said))
}

#[cfg(test)]
mod tests {
    use super::closes;

    #[test]
    fn the_proof_note_says_how_many_checks_close_it() {
        assert_eq!(closes(2), "closes itself when both pass");
        assert_eq!(closes(1), "closes itself when it passes");
        assert_eq!(closes(3), "closes itself when all 3 pass");
    }
}
