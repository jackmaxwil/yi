//! Section label and the key/value grid of an open row (record 1.6.9, 1.6.11).

use rune::{El, TextCase, TextStyle, col, row, spacer, text};
use yd_core::tokens::Tokens;

use crate::theme::look::{LABEL, META, colour};
use crate::theme::scale::{MEDIUM, SEMIBOLD};

/// 9.5 at weight 500, spaced .06 em, drawn upper case.
pub const SECTION: TextStyle = TextStyle {
    weight: MEDIUM,
    ..META
}
.tracking(0.57);
const COUNT: TextStyle = TextStyle {
    weight: SEMIBOLD,
    ..SECTION
};

/// A 20 tall section label with an optional bold count and a plain-case note at the right.
/// The 8 points above it (4 when first) are the caller's.
pub fn section(tokens: &Tokens, words: &str, count: Option<&str>, note: Option<&str>) -> El {
    row()
        .h(tokens.row)
        .px(tokens.gutter)
        .gap(6.0)
        .center_y()
        .child(
            text(words)
                .font(SECTION)
                .text_case(TextCase::Uppercase)
                .color(colour(tokens.faint)),
        )
        .children(count.map(|n| text(n).font(COUNT).color(colour(tokens.dim))))
        .children(note.map(|note| {
            row()
                .grow(1.0)
                .child(spacer())
                .child(text(note).font(META).color(colour(tokens.faint)))
        }))
}

/// Keys 56 wide in 9.5 faint, values 10.5 in bright, three pairs a line with 12 between.
pub fn key_values(tokens: &Tokens, pairs: &[(&str, &str)]) -> El {
    let pair = |(key, value): &(&str, &str)| {
        row()
            .grow(1.0)
            .min_w(0.0)
            .gap(6.0)
            .center_y()
            .child(
                row()
                    .w(56.0)
                    .child(text(*key).font(META).color(colour(tokens.faint))),
            )
            .child(
                text(*value)
                    .font(LABEL)
                    .color(colour(tokens.fg))
                    .truncate()
                    .min_w(0.0),
            )
    };
    col().gap(1.0).children(pairs.chunks(3).map(|line| {
        let filler = (line.len()..3).map(|_| row().grow(1.0).min_w(0.0));
        row()
            .gap(12.0)
            .children(line.iter().map(pair))
            .children(filler)
    }))
}
