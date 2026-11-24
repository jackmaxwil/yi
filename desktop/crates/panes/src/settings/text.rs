//! The settings as text, the right column: the same data as the rows, edited from either side.

use rune::{Color, El, Span, TextEdit, Tx, col, row, text, text_input};
use yd_core::ids::{PaneId, SettingKey};
use yd_core::model::Model;
use yd_core::settings::schema::{Setting, SettingValue};
use yd_core::settings::store;
use yd_core::settings::store::text::{Mark, Seen, read, render};
use yd_core::tokens::Tokens;
use yd_ui::a11y;
use yd_ui::kit::icons::glyph;
use yd_ui::theme::look::{CODE, LABEL, META, colour};
use yd_ui::win::Win;

use crate::settings::pane::act;
use crate::settings::pane::state::{Draft, Part, State};

pub const WIDTH: f32 = 300.0;

/// Mono 10.5 on 16, the record's text view.
const CODE_TEXT: rune::TextStyle = rune::TextStyle::new(10.5, 400.0).lh(16.0).family(CODE);

/// The text to show when the window's field does not have the keyboard: what was typed while
/// the model still says what it said then, else what the model says now.
pub fn shown(model: &Model, state: Option<&State>) -> String {
    let rendered = render(&store::all(model));
    match state.and_then(|state| state.draft.as_ref()) {
        Some(draft) if draft.rendered == rendered => draft.text.clone(),
        _ => rendered,
    }
}

/// Everything on the lines of `text` that sets something the model does not already hold.
fn changes(text: &str, rows: &[Setting]) -> Vec<(SettingKey, SettingValue)> {
    read(text, rows)
        .into_iter()
        .filter_map(|seen| match seen.mark {
            Mark::Set(key, value) => Some((key, value)),
            _ => None,
        })
        .collect()
}

/// Applies the lines of the window's field that are new since the last change, and keeps
/// the text so that it is shown as typed while nothing else changes the model.
pub fn typed(win: &mut Win, pane: PaneId) {
    let words = act::field(win, pane, Part::Text).text.clone();
    let Ok(hub) = win.hub.try_borrow() else {
        return;
    };
    let rows = store::all(&hub.model);
    let held = hub
        .panes
        .get::<State>(Some(pane))
        .and_then(|state| state.heard.clone());
    drop(hub);
    let now = changes(&words, &rows);
    let before = held.unwrap_or_else(|| changes(&render(&rows), &rows));
    let rendered = render(&rows);
    act::with_state(win, pane, |state| {
        state.draft = Some(Draft {
            text: words,
            rendered,
        });
        state.heard = Some(now.clone());
    });
    for (key, value) in now.into_iter().filter(|each| !before.contains(each)) {
        act::set_from_text(win, pane, &key, value);
    }
}

fn spans(seen: &[Seen], tokens: &Tokens) -> Vec<Span> {
    let ink = |c: yd_core::tokens::Rgba| -> Color { colour(c) };
    let mut out = Vec::new();
    for line in seen {
        let (start, end) = (line.bytes.start, line.bytes.end);
        if start >= end {
            continue;
        }
        let paint = |from: usize, to: usize, c: yd_core::tokens::Rgba| {
            let mut span = Span::new(from, to);
            span.color = Some(ink(c));
            span
        };
        match &line.mark {
            Mark::Blank => {}
            Mark::Comment => out.push(paint(start, end, tokens.comment)),
            Mark::Section => out.push(paint(start, end, tokens.fg)),
            Mark::Set(..) => {
                if let Some(at) = line.value_at {
                    out.push(paint(at, end, tokens.type_name));
                }
            }
            Mark::Bad(_) => {
                let mut span = paint(start, end, tokens.bad);
                span.underline = Some(ink(tokens.bad));
                out.push(span);
            }
        }
    }
    out
}

pub struct Drawn<'a> {
    pub pane: PaneId,
    pub key: &'a str,
    pub tokens: &'a Tokens,
    pub rows: &'a [Setting],
    /// The field to draw: [`crate::settings::pane::fields::drawn`].
    pub field: &'a TextEdit,
}

/// The first line that does not parse, with its number and why.
fn trouble(seen: &[Seen]) -> Option<String> {
    let bad: Vec<(usize, &str)> = seen
        .iter()
        .enumerate()
        .filter_map(|(n, line)| match &line.mark {
            Mark::Bad(why) => Some((n.saturating_add(1), why.as_str())),
            _ => None,
        })
        .collect();
    let (line, why) = bad.first()?;
    let more = bad.len().saturating_sub(1);
    let tail = if more > 0 {
        format!(" (and {more} more)")
    } else {
        String::new()
    };
    Some(format!("Line {line} is not applied: {why}{tail}"))
}

pub fn view(d: &Drawn<'_>) -> El {
    let (tokens, pane) = (d.tokens, d.pane);
    let seen = read(&d.field.text, d.rows);
    let warning = trouble(&seen);
    let field = text_input(d.field, CODE_TEXT)
        .key(format!("{}.text.input", d.key))
        .label("Settings as text")
        .w_full()
        .color(colour(tokens.dim))
        .caret(colour(tokens.accent), a11y::selection(tokens))
        .spans(spans(&seen, tokens))
        .on_edit(move |win: &mut Win| act::edit(win, pane, Part::Text))
        .on_input_change(move |win: &mut Win, _: &mut Tx| typed(win, pane))
        .on_submit(move |win: &mut Win, _: &mut Tx| {
            act::field(win, pane, Part::Text).insert("\n");
            typed(win, pane);
        });
    let bar = row()
        .key(format!("{}.text.title", d.key))
        .w_full()
        .h(tokens.bar)
        .px(tokens.gutter)
        .gap(5.0)
        .center_y()
        .child(glyph(&yd_ui::chrome::tabs::FILE, 11.0, tokens.faint))
        .child(text("As text").font(LABEL).color(colour(tokens.fg)));
    let said = warning.map_or_else(
        || {
            (
                "Edit here or on the left. Both stay in step.".to_owned(),
                tokens.faint,
            )
        },
        |why| (why, tokens.bad),
    );
    col()
        .key(format!("{}.text-column", d.key))
        .w(WIDTH)
        .h_full()
        .min_h(0.0)
        .child(bar)
        .child(
            col()
                .grow(1.0)
                .min_h(0.0)
                .w_full()
                .px(8.0)
                .py(6.0)
                .scroll(rune::ScrollSpec {
                    stick_bottom: false,
                })
                .child(field),
        )
        .child(
            row()
                .key(format!("{}.text.footer", d.key))
                .w_full()
                .min_h(tokens.row)
                .px(tokens.gutter)
                .py(3.0)
                .child(text(said.0).font(META).color(colour(said.1)).wrap()),
        )
}
