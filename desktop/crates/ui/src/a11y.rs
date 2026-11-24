//! Keyboard focus and the names assistive tools read: Tab stops, the one ring, selection.

use rune::{Color, Cx, El};
use yd_core::tokens::Tokens;

use crate::theme::look::colour;
use crate::theme::scale::{FOCUS_RING, SELECTION};
use crate::win::Win;

/// The ring's width in points, drawn just outside the control the keyboard is on.
pub const RING: f32 = 1.0;

/// Run first by every window's view: a pressable control is a Tab stop in reading order and
/// the keyboard's place is the theme's ring; a pointer press takes focus with no ring.
pub fn focus(cx: &mut Cx<Win>, tokens: &Tokens) {
    cx.web_focus();
    cx.focus_ring(RING, 0.0, colour(FOCUS_RING.of(tokens)));
}

/// A list the keyboard enters once: Tab reaches the picked row and the arrows rove the
/// rest, as in Rune's own sidebar. Pair each row with [`item`].
pub fn list(el: El, name: &str) -> El {
    el.role("list").label(name).focus_group()
}

/// A row of a [`list`], keyboard-reachable, drawing the same ring as any control: Rune
/// draws none of its own on a list's rows.
pub fn item(el: El, tokens: &Tokens, picked: bool) -> El {
    let ring = colour(FOCUS_RING.of(tokens));
    el.focusable(true)
        .focus_selected(picked)
        .focus_ring(RING, 0.0, ring)
}

/// The ring's colour, for a control that draws it inside its own edge.
pub fn ring_colour(tokens: &Tokens) -> Color {
    colour(FOCUS_RING.of(tokens))
}

/// The highlight of selected text, in a field or in a message.
pub fn selection(tokens: &Tokens) -> Color {
    colour(SELECTION.of(tokens))
}
