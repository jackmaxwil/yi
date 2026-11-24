//! The pane surface and its focused look.

use rune::{El, Shadow, col};
use yd_core::tokens::{Rgba, Tokens};

use crate::theme::look::{colour, white};
use crate::theme::scale::{HELD, RING_CARD, SHADOW_HELD, SHADOW_NEAR, TARGET_RING};

/// How a pane stands among the others while one of them is dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Look {
    Rest,
    /// The pane in the hand: lifted, opaque, with its shadow.
    Held,
    /// The pane under the pointer, where the held one would land.
    Target,
    /// Every other pane while one is held.
    Dimmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Surface {
    /// The pane the user is in takes the darker fill.
    pub focused: bool,
    /// Just chat: no fill and no lines, the pane sits on the window's glass.
    pub bare: bool,
    pub look: Look,
}

fn shadow(y: f32, blur: f32, spread: f32, ink: Rgba) -> Shadow {
    Shadow {
        y,
        blur,
        spread,
        color: colour(ink),
        ..Shadow::default()
    }
}

fn ring(width: f32, ink: Rgba) -> Shadow {
    Shadow {
        spread: width,
        color: colour(ink),
        inset: true,
        ..Shadow::default()
    }
}

/// A pane's surface, empty: flush, square, one hairline on its right and bottom edges so
/// neighbours share a line (record 2.6); lifted with its shadows while held (record 2.11.2).
pub fn surface(tokens: &Tokens, surface: Surface) -> El {
    let base = col();
    if surface.bare {
        return base;
    }
    if surface.look == Look::Held {
        return base
            .fill(colour(HELD.of(tokens)))
            .box_shadow(ring(1.0, RING_CARD.of(tokens)))
            .box_shadow(shadow(54.0, 80.0, -22.0, SHADOW_HELD.of(tokens)))
            .box_shadow(shadow(10.0, 18.0, 0.0, SHADOW_NEAR.of(tokens)));
    }
    let strength = if surface.focused {
        tokens.glass_pane_focus
    } else {
        tokens.glass_pane
    };
    let rest = base
        .fill(colour(tokens.pane_tint.alpha(strength)))
        .border_sides([0.0, 1.0, 1.0, 0.0], colour(white(tokens.lines)));
    match surface.look {
        Look::Rest | Look::Dimmed | Look::Held => rest,
        Look::Target => rest.box_shadow(ring(1.5, TARGET_RING.of(tokens))),
    }
}
