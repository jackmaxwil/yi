//! Chip and pill chip, with an optional icon and dropdown chevron (record 1.6.2).

use rune::{Cursor, El, Icon, row, text};
use yd_core::tokens::Tokens;

use crate::kit::button::PRESS_SCALE;
use crate::kit::icons::{DOWN, glyph};
use crate::theme::look::{LABEL, colour};
use crate::theme::scale::{PRESS, RADIUS_CHIP};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chip {
    /// Raised fill, dim words.
    Plain,
    /// A filter that is on: accent words on accent at 18 %.
    Pill,
}

/// A 19 tall chip. `menu` adds the 9 px down chevron of a dropdown.
pub fn chip(
    tokens: &Tokens,
    look: Chip,
    label: &str,
    lead: Option<&'static Icon>,
    menu: bool,
) -> El {
    let (fill, ink) = match look {
        Chip::Plain => (tokens.raised, tokens.dim),
        Chip::Pill => (tokens.accent.alpha(0.18), tokens.accent),
    };
    row()
        .h(19.0)
        .px(6.0)
        .gap(5.0)
        .center_y()
        .radius(RADIUS_CHIP.of(tokens))
        .fill(colour(fill))
        .cursor(Cursor::Pointer)
        .press(colour(PRESS.of(tokens)))
        .press_scale(PRESS_SCALE)
        .children(lead.map(|drawing| glyph(drawing, 9.0, ink)))
        .child(
            text(label)
                .font(LABEL)
                .color(colour(ink))
                .truncate()
                .min_w(0.0),
        )
        .children(menu.then(|| glyph(&DOWN, 9.0, ink)))
}
