//! Button: plain, primary, ghost; presses in and springs back (record 1.6.1, 1.8).

use rune::{Cursor, El, Shadow, TextStyle, row, text};
use yd_core::tokens::Tokens;

use crate::kit::key_hint::key_hint_in;
use crate::theme::look::{LABEL, NONE, colour};
use crate::theme::scale::{MEDIUM, ON_PRIMARY, PRESS, RADIUS_BUTTON, SHADOW_BUTTON};

/// 10.5 at weight 500.
pub const WORDS: TextStyle = TextStyle {
    weight: MEDIUM,
    ..LABEL
};

/// Press-in: scale .95 and the brightness drop, both springing back on release.
pub const PRESS_SCALE: f32 = 0.95;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Plain,
    Primary,
    Ghost,
}

/// A 20 tall button; a keyboard hint rides at .55 in the button's own ink. The caller adds
/// the press handler.
pub fn button(tokens: &Tokens, kind: Button, label: &str, hint: Option<&str>) -> El {
    shaped(tokens, kind, label, hint, false)
}

/// [`button`] whose words wrap when its row is narrower than they are, so none is cut.
pub fn wrapping(tokens: &Tokens, kind: Button, label: &str, hint: Option<&str>) -> El {
    shaped(tokens, kind, label, hint, true)
}

fn shaped(tokens: &Tokens, kind: Button, label: &str, hint: Option<&str>, wraps: bool) -> El {
    let (fill, line, ink) = match kind {
        Button::Plain => (tokens.panel, tokens.control_line, tokens.fg),
        Button::Primary => (tokens.fg, tokens.fg, ON_PRIMARY.of(tokens)),
        Button::Ghost => (NONE, NONE, tokens.dim),
    };
    let words = text(label).font(WORDS).color(colour(ink));
    let (words, body) = match wraps {
        true => (
            words.wrap().min_w(0.0).shrink(),
            row().min_h(20.0).min_w(0.0).shrink().py(2.0),
        ),
        false => (words, row().h(20.0)),
    };
    let body = body
        .px(7.0)
        .gap(6.0)
        .center_y()
        .radius(RADIUS_BUTTON.of(tokens))
        .fill(colour(fill))
        .border(1.0, colour(line))
        .cursor(Cursor::Pointer)
        .press(colour(PRESS.of(tokens)))
        .press_scale(PRESS_SCALE)
        .child(words)
        .children(hint.map(|keys| key_hint_in(tokens, ink, ink, keys).opacity(0.55)));
    match kind {
        Button::Plain | Button::Primary => body.box_shadow(Shadow {
            y: 1.0,
            blur: 2.0,
            color: colour(SHADOW_BUTTON.of(tokens)),
            ..Shadow::default()
        }),
        Button::Ghost => body,
    }
}
