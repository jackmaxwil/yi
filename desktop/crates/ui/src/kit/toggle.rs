//! Toggle (record 1.6.4): no motion between its states, as drawn.

use rune::{Cursor, El, el};
use yd_core::tokens::Tokens;

use crate::theme::look::colour;
use crate::theme::scale::{KNOB, RADIUS_BUTTON, TOGGLE_OFF};

/// Where the 8 px knob sits, off and on.
pub const KNOB_OFF: f32 = 1.5;
pub const KNOB_ON: f32 = 10.5;

/// A 20 by 11 switch; the caller adds the press handler. The knob is keyed `<key>.knob`.
pub fn toggle(tokens: &Tokens, key: &str, on: bool) -> El {
    let (track, left) = if on {
        (tokens.ok, KNOB_ON)
    } else {
        (TOGGLE_OFF.of(tokens), KNOB_OFF)
    };
    el().key(key)
        .w(20.0)
        .h(11.0)
        .radius(RADIUS_BUTTON.of(tokens))
        .fill(colour(track))
        .cursor(Cursor::Pointer)
        .child(
            el().key(format!("{key}.knob"))
                .absolute(left, 1.5)
                .size(8.0, 8.0)
                .radius(4.0)
                .fill(colour(KNOB.of(tokens))),
        )
}
