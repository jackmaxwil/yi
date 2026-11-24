//! Slider with its reading (record 1.6.6).

use rune::{Cursor, El, el, row, text};
use yd_core::tokens::Tokens;

use crate::theme::look::{CODE, LABEL, colour};
use crate::theme::scale::{RADIUS_TRACK, SLIDER_TRACK};

pub const TRACK: f32 = 96.0;

/// A share from 0 to 1; anything else is clamped, and NaN is 0.
pub fn share(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

/// A 96 by 4 track with a 12 px knob on the value and the reading beside it in mono. A press
/// or drag on the track gives `set` the share under the pointer. Parts are keyed under `key`.
pub fn slider<S: 'static>(
    tokens: &Tokens,
    key: &str,
    value: f32,
    reading: &str,
    set: impl Fn(&mut S, f32) + 'static,
) -> El {
    let at = share(value) * TRACK;
    let track = el()
        .key(format!("{key}.track"))
        .w(TRACK)
        .h(4.0)
        .radius(RADIUS_TRACK.of(tokens))
        .fill(colour(SLIDER_TRACK.of(tokens)))
        .cursor(Cursor::Pointer)
        .child(
            el().absolute(0.0, 0.0)
                .size(at, 4.0)
                .radius(RADIUS_TRACK.of(tokens))
                .fill(colour(tokens.dim)),
        )
        .child(
            el().key(format!("{key}.knob"))
                .absolute(at - 6.0, -4.0)
                .size(12.0, 12.0)
                .radius(6.0)
                .fill(colour(tokens.fg)),
        )
        .on_drag(move |state: &mut S, drag, _tx| {
            if drag.w > 0.0 {
                set(state, share(drag.x / drag.w));
            }
        });
    row().key(key).gap(8.0).center_y().child(track).child(
        text(reading)
            .font(LABEL.family(CODE))
            .color(colour(tokens.dim)),
    )
}
