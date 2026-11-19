//! Select: the closed control that opens a menu (record 1.6.5).

use rune::{Cursor, El, row, text};
use yd_core::tokens::Tokens;

use crate::kit::button::PRESS_SCALE;
use crate::kit::icons::{DOWN, glyph};
use crate::theme::look::{LABEL, colour};
use crate::theme::scale::{PRESS, RADIUS_CHIP};

/// A 20 tall select showing `chosen`; the caller opens the menu from its press handler.
pub fn select(tokens: &Tokens, chosen: &str) -> El {
    row()
        .h(20.0)
        .pl(8.0)
        .pr(6.0)
        .gap(6.0)
        .center_y()
        .radius(RADIUS_CHIP.of(tokens))
        .fill(colour(tokens.raised))
        .cursor(Cursor::Pointer)
        .press(colour(PRESS.of(tokens)))
        .press_scale(PRESS_SCALE)
        .child(text(chosen).font(LABEL).color(colour(tokens.fg)))
        .child(glyph(&DOWN, 9.0, tokens.faint))
}
