//! Sparkline (record 1.6.8).

use rune::{El, el, row};
use yd_core::tokens::Tokens;

use crate::theme::look::colour;

pub const HEIGHT: f32 = 9.0;

/// Bars 2 wide and 1 apart, bottom-aligned in 9 points, each as tall as its share of the
/// largest value. Negative and NaN values draw empty.
pub fn sparkline(tokens: &Tokens, series: &[f32]) -> El {
    let top = series
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(0.0_f32, f32::max);
    let tall = |value: f32| {
        if top > 0.0 && value.is_finite() {
            (value / top).clamp(0.0, 1.0) * HEIGHT
        } else {
            0.0
        }
    };
    row().h(HEIGHT).gap(1.0).children(
        series
            .iter()
            .map(|&value| col_bar(tall(value), colour(tokens.faint))),
    )
}

fn col_bar(tall: f32, ink: rune::Color) -> El {
    el().w(2.0).h(HEIGHT).child(
        el().absolute(0.0, HEIGHT - tall)
            .size(2.0, tall)
            .radius(1.0)
            .fill(ink),
    )
}
