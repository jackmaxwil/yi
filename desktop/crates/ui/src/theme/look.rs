//! Token values as Rune colours and text styles, and the tokens mapped onto Rune's `Kit`.

use std::hash::{DefaultHasher, Hash, Hasher};

use rune::{Color, Family, TextStyle};
use rune_kit::{Kit, PaletteSeed, Radius};
use yd_core::row::Tone;
use yd_core::tokens::{Rgba, ThemeValue, Tokens, rgba};

use crate::theme::scale::{
    HOVER, MEDIUM, RADIUS_BADGE, RADIUS_BUTTON, RADIUS_CARD, RADIUS_CHIP, RADIUS_MENU, SEMIBOLD,
};

/// The four sizes with their line heights (record 1.3.2).
pub const TITLE: TextStyle = TextStyle::new(12.5, 600.0).lh(17.5);
pub const BODY: TextStyle = TextStyle::new(11.5, 400.0).lh(16.1);
pub const LABEL: TextStyle = TextStyle::new(10.5, 400.0).lh(14.7);
pub const META: TextStyle = TextStyle::new(9.5, 400.0).lh(13.3);

/// The code face. Geist Mono is not installed here: swap this one value when its file ships.
pub const CODE: Family = Family::SystemMono;
/// Code and log lines: 10.5 on a 16 line (record 1.6.16).
pub const CODE_LINE: TextStyle = TextStyle::new(10.5, 400.0).lh(16.0).family(CODE);

/// Full-screen chat reads larger than the working scale on purpose (record 1.3.4).
pub const READ_GREETING: TextStyle = TextStyle::new(24.0, 650.0).tracking(-0.48);
pub const READ_TITLE: TextStyle = TextStyle::new(17.0, 650.0).tracking(-0.17);
pub const READ_QUOTE: TextStyle = TextStyle::new(14.5, 400.0).lh(21.0);
pub const READ_PROSE: TextStyle = TextStyle::new(13.5, 400.0).lh(21.6);
pub const READ_COMMAND: TextStyle = TextStyle::new(13.0, 400.0).lh(26.0).family(CODE);
pub const READ_SMALL: TextStyle = TextStyle::new(12.0, 400.0);

/// A number that moves when any token does, so what a view keeps is drawn again in a new theme.
pub fn stamp(tokens: &Tokens) -> u64 {
    let mut hasher = DefaultHasher::new();
    let named = tokens.values();
    let named = named.iter().map(|(name, value)| (*name, value));
    let extra = tokens
        .extra
        .iter()
        .map(|(name, value)| (name.as_str(), value));
    for (name, value) in named.chain(extra) {
        name.hash(&mut hasher);
        match value {
            ThemeValue::Flag(flag) => flag.hash(&mut hasher),
            ThemeValue::Number(number) => number.to_bits().hash(&mut hasher),
            ThemeValue::Text(text) => text.hash(&mut hasher),
        }
    }
    hasher.finish()
}

/// A token colour in Rune's linear light. Rune's own reader is the safe one: it never panics.
pub fn colour(value: Rgba) -> Color {
    rune::color::hex(&value.hex()).map_or(Color::TRANSPARENT, |colour| colour.alpha(value.a))
}

/// White at strength `a`: the record's tracks, fills and hairlines.
pub const fn white(a: f32) -> Rgba {
    rgba(255, 255, 255, a)
}

/// Black at strength `a`: the record's shadows.
pub const fn black(a: f32) -> Rgba {
    rgba(0, 0, 0, a)
}

/// Nothing: a fill or line that is not drawn, which no theme colours.
pub const NONE: Rgba = white(0.0);

/// The colour a role stands for (record 1.1.2).
pub fn tone(tokens: &Tokens, tone: Tone) -> Rgba {
    match tone {
        Tone::Fg => tokens.fg,
        Tone::Dim => tokens.dim,
        Tone::Faint => tokens.faint,
        Tone::Accent => tokens.accent,
        Tone::Need => tokens.need,
        Tone::Ok => tokens.ok,
        Tone::Bad => tokens.bad,
        Tone::Warn => tokens.warn,
        Tone::Model => tokens.model,
        Tone::Context => tokens.context,
    }
}

/// Rune's stock components in the app's look: its strengths, roles, radii and type.
pub fn kit(tokens: &Tokens) -> Kit {
    let mut kit = Kit::from_palette(PaletteSeed {
        bg: colour(tokens.window),
        fg: colour(tokens.fg),
        accent: colour(tokens.accent),
    });
    let c = &mut kit.c;
    (c.text_base, c.text_strong, c.text_muted) =
        (colour(tokens.fg), colour(tokens.fg), colour(tokens.dim));
    (c.text_faint, c.text_weak, c.text_accent) = (
        colour(tokens.faint),
        colour(tokens.faint),
        colour(tokens.accent),
    );
    (c.icon_base, c.icon_muted, c.icon_faint) =
        (colour(tokens.dim), colour(tokens.dim), colour(tokens.faint));
    (c.border_base, c.border_muted) = (colour(tokens.control_line), colour(white(tokens.lines)));
    (c.hover, c.surface_raised) = (colour(HOVER.of(tokens)), colour(tokens.raised));
    (c.success.fg, c.danger.fg, c.warning.fg) =
        (colour(tokens.ok), colour(tokens.bad), colour(tokens.warn));
    c.info.fg = colour(tokens.accent);
    kit.radius = Radius {
        xs: RADIUS_BADGE.of(tokens),
        sm: RADIUS_CHIP.of(tokens),
        md: RADIUS_BUTTON.of(tokens),
        lg: RADIUS_CARD.of(tokens),
        xl: RADIUS_MENU.of(tokens),
    };
    let ty = &mut kit.ty;
    (ty.ui, ty.ui_medium) = (
        LABEL,
        TextStyle {
            weight: MEDIUM,
            ..LABEL
        },
    );
    (ty.body, ty.body_medium) = (
        BODY,
        TextStyle {
            weight: SEMIBOLD,
            ..BODY
        },
    );
    (ty.small, ty.small_medium) = (
        META,
        TextStyle {
            weight: MEDIUM,
            ..META
        },
    );
    ty.mono = CODE_LINE;
    kit
}
