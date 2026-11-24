//! Keyboard hint keycap (record 1.6.7).

use rune::{El, TextStyle, row, text};
use yd_core::tokens::{Rgba, Tokens};

use crate::theme::look::{META, colour};
use crate::theme::scale::{MEDIUM, RADIUS_HINT};

/// 9.5 at weight 500 on a line of 1, inside a 1 px ring: about 14 tall.
pub const HINT: TextStyle = TextStyle {
    weight: MEDIUM,
    ..META
}
.lh(9.5);

/// The role every keycap carries, so a test can check each one drawn against the map.
pub const KEYCAP: &str = "keycap";

/// A keycap such as "⌘K": faint text in a faint ring.
pub fn key_hint(tokens: &Tokens, keys: &str) -> El {
    key_hint_in(tokens, tokens.faint, tokens.control_line, keys)
}

/// A keycap in another ink, as inside a button: text and ring in `ink`, ring of `line`.
pub fn key_hint_in(tokens: &Tokens, ink: Rgba, line: Rgba, keys: &str) -> El {
    row()
        .role(KEYCAP)
        .h(14.0)
        .px(3.0)
        .center()
        .radius(RADIUS_HINT.of(tokens))
        .border(1.0, colour(line))
        .child(text(keys).font(HINT).color(colour(ink)))
}
