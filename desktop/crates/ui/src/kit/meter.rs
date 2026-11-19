//! Meter and bar, with the 65 and 85 percent thresholds (record 1.1.3).

use rune::{El, el};
use yd_core::tokens::{Rgba, Tokens};

use crate::kit::slider::share;
use crate::theme::look::colour;
use crate::theme::scale::{METER_TRACK, RADIUS_TRACK};

pub const METER_W: f32 = 34.0;

/// The meter's fill: normal up to 65 %, needs-you above, bad above 85 %.
pub fn meter_colour(tokens: &Tokens, ratio: f32) -> Rgba {
    let ratio = share(ratio);
    if ratio > 0.85 {
        tokens.bad
    } else if ratio > 0.65 {
        tokens.need
    } else {
        tokens.meter
    }
}

/// A 34 by 4 meter filled to `ratio`; the fill is keyed `<key>.fill`.
pub fn meter(tokens: &Tokens, key: &str, ratio: f32) -> El {
    let radius = RADIUS_TRACK.of(tokens);
    el().key(key)
        .size(METER_W, 4.0)
        .radius(radius)
        .fill(colour(METER_TRACK.of(tokens)))
        .child(
            el().key(format!("{key}.fill"))
                .absolute(0.0, 0.0)
                .size(share(ratio) * METER_W, 4.0)
                .radius(radius)
                .fill(colour(meter_colour(tokens, ratio))),
        )
}

/// A 6 tall bar `width` wide, filled to `ratio`; `heavy` draws it bad.
pub fn bar(tokens: &Tokens, width: f32, ratio: f32, heavy: bool) -> El {
    let ink = if heavy { tokens.bad } else { tokens.meter };
    el().size(share(ratio) * width.max(0.0), 6.0)
        .radius(3.0)
        .fill(colour(ink))
}
