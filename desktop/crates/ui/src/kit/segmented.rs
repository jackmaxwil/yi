//! Segmented control (record 1.6.3).

use rune::{Cursor, El, row, text};
use yd_core::tokens::Tokens;

use crate::kit::button::PRESS_SCALE;
use crate::theme::look::{LABEL, NONE, colour};
use crate::theme::scale::{PRESS, RADIUS_BUTTON, RADIUS_CHIP, SEGMENT_ON};

/// A 21 tall track of 17 tall items; `pick` gets the index pressed. Items are keyed
/// `<key>.<index>`.
pub fn segmented<S: 'static>(
    tokens: &Tokens,
    key: &str,
    items: &[&str],
    selected: usize,
    pick: impl Fn(&mut S, usize) + Clone + 'static,
) -> El {
    let item = |(index, words): (usize, &&str)| {
        let on = index == selected;
        let (fill, ink) = if on {
            (SEGMENT_ON.of(tokens), tokens.fg)
        } else {
            (NONE, tokens.dim)
        };
        let pick = pick.clone();
        row()
            .key(format!("{key}.{index}"))
            .h(17.0)
            .px(7.0)
            .center()
            .radius(RADIUS_CHIP.of(tokens))
            .fill(colour(fill))
            .cursor(Cursor::Pointer)
            .press(colour(PRESS.of(tokens)))
            .press_scale(PRESS_SCALE)
            .child(text(*words).font(LABEL).color(colour(ink)))
            .on_press(move |state: &mut S| pick(state, index))
    };
    row()
        .key(key)
        .h(21.0)
        .padding(2.0)
        .center_y()
        .radius(RADIUS_BUTTON.of(tokens))
        .fill(colour(tokens.raised))
        .children(items.iter().enumerate().map(item))
}
