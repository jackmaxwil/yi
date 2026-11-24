//! A description drawn with the app's own components, so a plugin follows the theme.

use rune::{El, Shape, TextCase, TextStyle, Tx, col, el, row, text};
use yd_core::row::Tone;
use yd_core::tokens::Tokens;
use yd_ui::kit::button::{Button, button};
use yd_ui::kit::label::section;
use yd_ui::theme::look::{CODE_LINE, LABEL, META, colour, tone};
use yd_ui::win::Win;

use crate::script_pane::host::{Cell, Piece};

const NUMBER: TextStyle = TextStyle::new(17.0, 650.0).lh(18.7);
const NAME: TextStyle = TextStyle::new(10.5, 600.0).lh(14.7);
const STAT_GAP: f32 = 8.0;
const SPARK: f32 = 44.0;
/// Columns after a table's first are this wide; the first takes what is left.
const COLUMN: f32 = 70.0;

/// The key part a label or a cell gives: dots would read as more parts.
pub fn part(words: &str) -> String {
    words.replace(['.', ' ', '/'], "-")
}

/// Every piece, stats that follow each other side by side as tiles.
pub fn pieces(tokens: &Tokens, key: &str, plugin: &str, all: &[Piece]) -> El {
    let mut out = col().w_full().gap(4.0);
    let mut tiles: Vec<El> = Vec::new();
    for piece in all {
        if let Piece::Stat { label, value, tone } = piece {
            tiles.push(stat(tokens, key, label, value, *tone));
            continue;
        }
        if !tiles.is_empty() {
            out = out.child(stats(std::mem::take(&mut tiles)));
        }
        out = out.child(one(tokens, key, plugin, piece));
    }
    if !tiles.is_empty() {
        out = out.child(stats(tiles));
    }
    out
}

fn stats(tiles: Vec<El>) -> El {
    row()
        .w_full()
        .px(8.0)
        .pt(8.0)
        .pb(4.0)
        .gap(STAT_GAP)
        .children(tiles)
}

fn stat(tokens: &Tokens, key: &str, label: &str, value: &str, role: Option<Tone>) -> El {
    let ink = role.map_or(tokens.fg, |role| tone(tokens, role));
    let tile = col()
        .key(format!("{key}.stat.{}", part(label)))
        .grow(1.0)
        .min_w(0.0)
        .h(45.0)
        .px(8.0)
        .py(6.0)
        .radius(6.0)
        .fill(colour(yd_ui::theme::scale::STAT.of(tokens)))
        .child(text(value).font(NUMBER).color(colour(ink)))
        .child(
            text(label)
                .font(META.tracking(0.57))
                .text_case(TextCase::Uppercase)
                .color(colour(tokens.faint)),
        );
    match role {
        Some(role) => tile.border(1.0, colour(tone(tokens, role).alpha(0.5))),
        None => tile,
    }
}

fn one(tokens: &Tokens, key: &str, plugin: &str, piece: &Piece) -> El {
    match piece {
        Piece::Stat { .. } => col(),
        Piece::Spark { series, .. } => spark(tokens, key, series),
        Piece::Table {
            title,
            note,
            columns,
            rows,
        } => table(
            tokens,
            key,
            title.as_deref(),
            note.as_deref(),
            columns,
            rows,
        ),
        Piece::List { title, items } => {
            let lines = items.iter().map(|item| {
                line(
                    tokens,
                    format!("{key}.item.{}", part(&item.text)),
                    item,
                    0.0,
                )
            });
            titled(tokens, title.as_deref(), None).children(lines)
        }
        Piece::Tree { title, lines } => {
            let shown = lines.iter().map(|(depth, words)| {
                let cell = Cell {
                    text: words.clone(),
                    tone: None,
                };
                let inset = 12.0 * (*depth).min(16) as f32;
                line(tokens, format!("{key}.node.{}", part(words)), &cell, inset)
            });
            titled(tokens, title.as_deref(), None).children(shown)
        }
        Piece::Button { title, id } => {
            let (plugin, id) = (plugin.to_owned(), id.clone());
            let press = move |win: &mut Win, tx: &mut Tx| super::press(win, tx, &plugin, &id);
            row().px(8.0).py(4.0).child(
                button(tokens, Button::Plain, title, None)
                    .key(format!("{key}.button.{}", part(title)))
                    .on_press_tx(press),
            )
        }
        Piece::Code(words) => block(tokens, words.lines(), tokens.fg),
        Piece::Log(lines) => block(tokens, lines.iter().map(String::as_str), tokens.dim),
    }
}

/// A line through the series, the highest value near the top; the box stretches to the pane.
fn spark(tokens: &Tokens, key: &str, series: &[f32]) -> El {
    const WIDE: f32 = 300.0;
    let top = series
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(0.0_f32, f32::max);
    let last = series.len().saturating_sub(1).max(1) as f32;
    let points: Vec<[f32; 2]> = series
        .iter()
        .enumerate()
        .map(|(at, v)| {
            let share = if top > 0.0 && v.is_finite() {
                (v / top).clamp(0.0, 1.0)
            } else {
                0.0
            };
            [at as f32 / last * WIDE, SPARK - 2.0 - share * (SPARK - 4.0)]
        })
        .collect();
    let line = Shape::new()
        .stroke(points, 1.5, colour(tokens.dim))
        .view_box(WIDE, SPARK);
    row().w_full().px(8.0).child(
        el().key(format!("{key}.spark"))
            .w_full()
            .h(SPARK)
            .shape(line),
    )
}

fn titled(tokens: &Tokens, title: Option<&str>, note: Option<&str>) -> El {
    col()
        .w_full()
        .children(title.map(|title| section(tokens, title, None, note)))
}

fn line(tokens: &Tokens, key: String, cell: &Cell, inset: f32) -> El {
    let ink = cell.tone.map_or(tokens.dim, |role| tone(tokens, role));
    row()
        .key(key)
        .h(tokens.row)
        .px(tokens.gutter)
        .pl(tokens.gutter + inset)
        .center_y()
        .child(
            text(cell.text.as_str())
                .font(LABEL)
                .color(colour(ink))
                .truncate(),
        )
}

fn block<'a>(
    tokens: &Tokens,
    lines: impl Iterator<Item = &'a str>,
    ink: yd_core::tokens::Rgba,
) -> El {
    let shown = lines.map(|words| text(words).font(CODE_LINE).color(colour(ink)));
    col().w_full().px(tokens.gutter).py(4.0).children(shown)
}

fn table(
    tokens: &Tokens,
    key: &str,
    title: Option<&str>,
    note: Option<&str>,
    columns: &[String],
    rows: &[Vec<Cell>],
) -> El {
    let header = row()
        .key(format!("{key}.table-header"))
        .w_full()
        .h(tokens.row)
        .px(tokens.gutter)
        .center_y()
        .children(columns.iter().enumerate().map(|(at, name)| {
            let title = text(capital(name)).font(META).color(colour(tokens.faint));
            sized(row().child(title), at, columns.len())
        }));
    let body = rows.iter().map(|cells| {
        let id = cells.first().map(|c| part(&c.text)).unwrap_or_default();
        row()
            .key(format!("{key}.row.{id}"))
            .w_full()
            .h(tokens.row)
            .px(tokens.gutter)
            .center_y()
            .children(cells.iter().enumerate().map(|(at, cell)| {
                let ink = cell.tone.map_or(tokens.dim, |role| tone(tokens, role));
                let font = if at == 0 { NAME } else { LABEL };
                let words = text(cell.text.as_str())
                    .font(font)
                    .color(colour(ink))
                    .truncate();
                sized(row().child(words), at, cells.len())
            }))
    });
    titled(tokens, title, note).child(header).children(body)
}

/// The first column takes what is left; the last of several is set to the right.
fn sized(cell: El, at: usize, of: usize) -> El {
    match at {
        0 => cell.grow(1.0).min_w(0.0),
        _ if at.saturating_add(1) == of => cell.w(COLUMN * 0.5).justify_end(),
        _ => cell.w(COLUMN),
    }
}

/// A column's field name as its title: `tries` is "Tries".
fn capital(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}
