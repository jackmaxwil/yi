//! Hover card (record 1.6.14). A plain tooltip is the system's: `El::help`.

use rune::{El, Shadow, TextStyle, col, row, text};
use yd_core::tokens::Tokens;

use crate::theme::look::{CODE, LABEL, META, colour};
use crate::theme::scale::{CARD, LINE_SOFT, RADIUS_CARD, RING_CARD, SHADOW_MENU};

pub const WIDTH: f32 = 282.0;
const PROSE: TextStyle = LABEL.lh(14.7);
const SIGNATURE: TextStyle = LABEL.lh(15.0).family(CODE);

/// A hover card: a signature in mono, prose in dim, and a row of accent links under a hairline.
/// The app owns when it opens and closes.
pub fn hover_card(tokens: &Tokens, signature: &str, prose: &str, links: &[&str]) -> El {
    let link_row = (!links.is_empty()).then(|| {
        row()
            .gap(11.0)
            .pt(3.0)
            .border_sides([1.0, 0.0, 0.0, 0.0], colour(LINE_SOFT.of(tokens)))
            .children(
                links
                    .iter()
                    .map(|words| text(*words).font(META).color(colour(tokens.accent))),
            )
    });
    col()
        .w(WIDTH)
        .px(8.0)
        .py(6.0)
        .gap(4.0)
        .radius(RADIUS_CARD.of(tokens))
        .fill(colour(CARD.of(tokens)))
        .box_shadow(Shadow {
            spread: 1.0,
            color: colour(RING_CARD.of(tokens)),
            ..Shadow::default()
        })
        .box_shadow(Shadow {
            y: 14.0,
            blur: 30.0,
            spread: -8.0,
            color: colour(SHADOW_MENU.of(tokens)),
            ..Shadow::default()
        })
        .child(
            text(signature)
                .font(SIGNATURE)
                .color(colour(tokens.fg))
                .wrap(),
        )
        .child(text(prose).font(PROSE).color(colour(tokens.dim)).wrap())
        .children(link_row)
}
