//! Pane header bar and footer line (record 1.6.12, 5.1.4).

use rune::{El, TextStyle, row, spacer, text};
use yd_core::tokens::Tokens;

use crate::theme::look::{LABEL, colour, white};
use crate::theme::scale::SEMIBOLD;

/// The first, bright item of a header bar.
pub const LEAD: TextStyle = TextStyle {
    weight: SEMIBOLD,
    ..LABEL
};

/// The bold lead of a header bar: 10.5 at 600 in bright.
pub fn lead(tokens: &Tokens, words: &str) -> El {
    text(words).font(LEAD).color(colour(tokens.fg))
}

fn bar(tokens: &Tokens, height: f32, gap: f32, left: Vec<El>, right: Vec<El>) -> El {
    row()
        .w_full()
        .h(height)
        .px(tokens.gutter)
        .gap(gap)
        .center_y()
        .clip_rounded(0.0)
        .children(left)
        .child(spacer())
        .child(row().gap(6.0).center_y().children(right))
}

/// A 22 tall header bar with an inner hairline under it; `right` is pushed to the right.
pub fn header(tokens: &Tokens, left: Vec<El>, right: Vec<El>) -> El {
    bar(tokens, tokens.bar, 6.0, left, right)
        .border_sides([0.0, 0.0, 1.0, 0.0], colour(white(tokens.inner_lines)))
}

/// A 20 tall footer line with an inner hairline over it; `right` is pushed to the right.
pub fn footer(tokens: &Tokens, left: Vec<El>, right: Vec<El>) -> El {
    bar(tokens, tokens.row, 10.0, left, right)
        .border_sides([1.0, 0.0, 0.0, 0.0], colour(white(tokens.inner_lines)))
}
