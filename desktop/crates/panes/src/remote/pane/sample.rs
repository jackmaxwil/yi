//! The remote screen's words and windows as elements over its picture: what a picture of text
//! cannot give right-click and pick. Its colours are the remote desktop's own, not the app's.

use rune::{El, TextStyle, el, row, spacer, text};
use yd_core::feed::Table;
use yd_core::row::{Row, Rows};
use yd_core::tokens::{Rgba, Tokens, rgb};
use yd_ui::theme::look::{CODE, colour};

const BAR: Rgba = rgb(0x15, 0x18, 0x1d);
const BAR_INK: Rgba = rgb(0xd6, 0xd8, 0xdc);
const APP_FACE: Rgba = rgb(0xf7, 0xf7, 0xf8);
const APP_BAR: Rgba = rgb(0xdc, 0xdd, 0xe0);
const APP_INK: Rgba = rgb(0x3c, 0x3f, 0x44);
const TERMINAL_FACE: Rgba = rgb(0x10, 0x12, 0x16);
const TERMINAL_BAR: Rgba = rgb(0x2a, 0x2d, 0x33);
const TERMINAL_TITLE: Rgba = rgb(0xc9, 0xcb, 0xd0);
const LOG_INK: Rgba = rgb(0xb9, 0xbc, 0xc2);
const LOG_WARN: Rgba = rgb(0xe3, 0xaa, 0x6d);
const ROW_INK: Rgba = rgb(0x1a, 0x1b, 0x1d);
const STATUS_INK: Rgba = rgb(0x5a, 0x5d, 0x63);
const LIMIT_INK: Rgba = rgb(0xb7, 0x77, 0x00);
const TRACK: Rgba = rgb(0xdc, 0xdd, 0xe0);
const FILLED: Rgba = rgb(0x2f, 0x9a, 0x57);
const LIMITED: Rgba = rgb(0xd9, 0x9a, 0x1c);
const AGENT: Rgba = rgb(0x7a, 0x4f, 0xd0);
const WHITE: Rgba = rgb(0xff, 0xff, 0xff);

fn number(item: &Row, field: &str) -> f32 {
    item.words(field)
        .and_then(|w| w.parse().ok())
        .unwrap_or(0.0)
}

fn words(item: &Row, field: &str) -> String {
    item.words(field).unwrap_or_default().to_owned()
}

fn font(size: f32, weight: f32, s: f32) -> TextStyle {
    TextStyle::new(size * s, weight).lh(size * s * 1.4)
}

fn rows_of<'a>(table: &'a Table, kind: &'a str) -> impl Iterator<Item = &'a Row> {
    let all = (0..table.len()).filter_map(move |at| table.get(at));
    all.filter(move |item| item.words("kind") == Some(kind))
}

fn at(item: &Row, s: f32) -> El {
    let (x, y) = (number(item, "x") * s, number(item, "y") * s);
    el().w(number(item, "w") * s)
        .h(number(item, "h") * s)
        .absolute(x, y)
}

fn top_bar(table: &Table, s: f32) -> Option<El> {
    let bars: Vec<&Row> = rows_of(table, "bar").collect();
    let first = bars.first()?;
    let word = |item: &&Row| {
        text(words(item, "name"))
            .font(font(10.5, 400.0, s))
            .color(colour(BAR_INK))
    };
    let mut line = row()
        .w(number(first, "w") * s)
        .h(number(first, "h") * s)
        .px(10.0 * s)
        .center_y()
        .fill(colour(BAR))
        .absolute(0.0, 0.0);
    for (n, bar) in bars.iter().enumerate() {
        if n > 0 {
            line = line.child(spacer());
        }
        line = line.child(word(bar));
    }
    Some(line)
}

fn window(tokens: &Tokens, key: &str, item: &Row, s: f32) -> El {
    let dark = item.id.as_str().ends_with("terminal");
    let (face, bar, ink) = if dark {
        (TERMINAL_FACE, TERMINAL_BAR, TERMINAL_TITLE)
    } else {
        (APP_FACE, APP_BAR, APP_INK)
    };
    let _ = tokens;
    let title = row().w_full().h(24.0 * s).center().fill(colour(bar)).child(
        text(words(item, "name"))
            .font(font(10.5, 600.0, s))
            .color(colour(ink)),
    );
    at(item, s)
        .key(format!("{key}.screen.{}", item.id.as_str()))
        .radius(7.0 * s)
        .clip_rounded(7.0 * s)
        .fill(colour(face))
        .child(title)
}

fn upload(key: &str, item: &Row, s: f32) -> El {
    let limited = words(item, "status").starts_with("Retrying");
    let status_ink = if limited { LIMIT_INK } else { STATUS_INK };
    let (w, fill) = (number(item, "w"), number(item, "fill"));
    let weight = if limited { 600.0 } else { 400.0 };
    let name = text(words(item, "name"))
        .font(font(11.0, 550.0, s).family(CODE))
        .color(colour(ROW_INK));
    let status = text(words(item, "status"))
        .font(font(11.0, weight, s))
        .color(colour(status_ink));
    let line = row_line(name, status, 14.0 * s, 9.0 * s);
    let track = el()
        .w((w - 28.0) * s)
        .h(4.0 * s)
        .radius(2.0 * s)
        .fill(colour(TRACK))
        .absolute(14.0 * s, 28.0 * s);
    let ink = if limited { LIMITED } else { FILLED };
    let done = el()
        .w((w - 28.0) * s * fill / 100.0)
        .h(4.0 * s)
        .radius(2.0 * s)
        .fill(colour(ink))
        .absolute(14.0 * s, 28.0 * s);
    at(item, s)
        .key(format!("{key}.screen.{}", item.id.as_str()))
        .child(line)
        .child(track)
        .child(done)
}

fn row_line(name: El, status: El, left: f32, top: f32) -> El {
    row()
        .w_full()
        .px(left)
        .py(top)
        .child(name)
        .child(spacer())
        .child(status)
}

fn log(item: &Row, s: f32) -> El {
    let warn = item.words("warn") == Some("yes");
    let ink = if warn { LOG_WARN } else { LOG_INK };
    let style = TextStyle::new(10.5 * s, 400.0).lh(17.0 * s).family(CODE);
    at(item, s).child(text(words(item, "name")).font(style).color(colour(ink)))
}

fn pointer(item: &Row, s: f32) -> El {
    let tag = text(words(item, "name"))
        .font(font(10.0, 600.0, s))
        .color(colour(WHITE));
    let (x, y) = (number(item, "x") * s, number(item, "y") * s);
    row()
        .px(6.0 * s)
        .radius(9.0 * s)
        .fill(colour(AGENT))
        .child(tag)
        .absolute(x + 12.0 * s, y + 12.0 * s)
}

/// What a window holds: its upload rows and its log lines.
fn held<'a>(table: &'a Table, window: &'a Row) -> impl Iterator<Item = &'a Row> {
    let id = window
        .id
        .as_str()
        .strip_prefix("window:")
        .unwrap_or_default();
    let all = (0..table.len()).filter_map(move |at| table.get(at));
    all.filter(move |item| item.words("parent") == Some(id))
}

/// The screen's words and windows at `s` points a pixel, as one layer that takes no input.
/// Each window is followed by what it holds, so a later window covers an earlier one.
pub fn overlay(tokens: &Tokens, key: &str, table: Option<&Table>, s: f32) -> El {
    let layer = el().w_full().h_full().inert().absolute(0.0, 0.0);
    let Some(table) = table else {
        return layer;
    };
    let windows = rows_of(table, "window").flat_map(|win| {
        let inside = held(table, win).map(|item| match item.words("kind") {
            Some("line") => log(item, s),
            _ => upload(key, item, s),
        });
        std::iter::once(window(tokens, key, win, s)).chain(inside)
    });
    layer
        .children(top_bar(table, s))
        .children(windows)
        .children(rows_of(table, "pointer").map(|r| pointer(r, s)))
}
