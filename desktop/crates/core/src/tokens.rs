//! One owned set of design values. A theme is a small file that sets some of them by name;
//! the rest keep the base, Yi Dark (record 1.1, 1.4, 7.4).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::ThemeId;
use crate::row::ShapeError;

/// A colour as the record writes it: sRGB bytes and an opacity from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f32,
}

pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    Rgba { r, g, b, a: 1.0 }
}

pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Rgba {
    Rgba { r, g, b, a }
}

impl Rgba {
    /// `#rrggbb`, `#rrggbbaa`, or `rgba(r, g, b, a)` with the opacity from 0 to 1.
    pub fn parse(text: &str) -> Option<Rgba> {
        if let Some(inner) = text
            .strip_prefix("rgba(")
            .and_then(|rest| rest.strip_suffix(')'))
        {
            let mut parts = inner.split(',').map(str::trim);
            let mut byte = || parts.next()?.parse::<u8>().ok();
            let (r, g, b) = (byte()?, byte()?, byte()?);
            let a = parts
                .next()?
                .parse::<f32>()
                .ok()
                .filter(|a| (0.0..=1.0).contains(a))?;
            return parts.next().is_none().then_some(rgba(r, g, b, a));
        }
        let hex = text.strip_prefix('#')?;
        let byte = |at: usize| u8::from_str_radix(hex.get(at..at.checked_add(2)?)?, 16).ok();
        match hex.len() {
            6 => Some(rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(rgba(
                byte(0)?,
                byte(2)?,
                byte(4)?,
                f32::from(byte(6)?) / 255.0,
            )),
            _ => None,
        }
    }

    /// `#rrggbb`: the colour without its opacity.
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    /// The form a theme file holds: `#rrggbb`, or `rgba(..)` when it is see-through.
    pub fn text(self) -> String {
        if self.a == 1.0 {
            self.hex()
        } else {
            format!("rgba({}, {}, {}, {})", self.r, self.g, self.b, self.a)
        }
    }

    pub const fn alpha(self, a: f32) -> Rgba {
        Rgba { a, ..self }
    }
}

/// One value in a theme file: a colour or a font as text, a size or strength as a number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ThemeValue {
    Flag(bool),
    Number(f64),
    Text(String),
}

/// Why a theme value was not taken. The token keeps the value it had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("this token takes a colour written #rrggbb or rgba(r, g, b, a)")]
    NotAColour,
    #[error("this token takes a number")]
    NotANumber,
    #[error("this token takes text")]
    NotText,
    #[error("this token takes true or false")]
    NotAFlag,
}

impl ThemeValue {
    fn colour(&self) -> Result<Rgba, TokenError> {
        match self {
            ThemeValue::Text(text) => Rgba::parse(text).ok_or(TokenError::NotAColour),
            ThemeValue::Flag(_) | ThemeValue::Number(_) => Err(TokenError::NotAColour),
        }
    }

    fn number(&self) -> Result<f32, TokenError> {
        match self {
            // A number too large for an f32 narrows to infinity, which is refused like text.
            ThemeValue::Number(n) => Some(*n as f32)
                .filter(|narrow| narrow.is_finite())
                .ok_or(TokenError::NotANumber),
            ThemeValue::Flag(_) | ThemeValue::Text(_) => Err(TokenError::NotANumber),
        }
    }

    fn text(&self) -> Result<String, TokenError> {
        match self {
            ThemeValue::Text(text) => Ok(text.clone()),
            ThemeValue::Flag(_) | ThemeValue::Number(_) => Err(TokenError::NotText),
        }
    }

    fn flag(&self) -> Result<bool, TokenError> {
        match self {
            ThemeValue::Flag(flag) => Ok(*flag),
            ThemeValue::Number(_) | ThemeValue::Text(_) => Err(TokenError::NotAFlag),
        }
    }
}

/// A theme file: `{"id", "name", "note", "dark", "values": {"<token name>": value}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeFile {
    pub id: ThemeId,
    pub name: String,
    #[serde(default)]
    pub note: String,
    /// False for a light theme, for "Follow light and dark from the Mac".
    pub dark: bool,
    /// The values this theme sets; every other token keeps the base.
    #[serde(default)]
    pub values: BTreeMap<String, ThemeValue>,
    /// Keys this app does not know, kept so that a write gives them back.
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// The mockup's theme files name the three glass strengths `surface.*`; the app's `glass.*`.
const ALIASES: [(&str, &str); 3] = [
    ("surface.pane", "glass.pane"),
    ("surface.pane_active", "glass.pane_in_use"),
    ("surface.window", "glass.window"),
];

/// The token a name in a theme file stands for: an alias gives the app's own name.
pub fn token_name(name: &str) -> &str {
    let alias = ALIASES.iter().find(|(alias, _)| *alias == name);
    alias.map_or(name, |(_, token)| token)
}

impl ThemeFile {
    pub fn parse(text: &str) -> Result<ThemeFile, ShapeError> {
        Ok(serde_json::from_str(text)?)
    }

    /// The file's text, made once for one write: what [`ThemeFile::parse`] reads back.
    pub fn to_text(&self) -> Result<String, ShapeError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Sets one value, under the name the file already writes that token by.
    pub fn set(&mut self, name: &str, value: ThemeValue) {
        let token = token_name(name);
        let written = self.values.keys().find(|key| token_name(key) == token);
        let key = written.cloned().unwrap_or_else(|| name.to_owned());
        self.values.insert(key, value);
    }
}

macro_rules! tokens {
    (
        colours { $($c:ident $cn:literal = $cv:expr),+ $(,)? }
        numbers { $($n:ident $nn:literal = $nv:expr),+ $(,)? }
        texts { $($t:ident $tn:literal = $tv:literal),+ $(,)? }
        flags { $($f:ident $fl:literal = $fv:literal),+ $(,)? }
    ) => {
        /// Every design value the app reads. The defaults are Yi Dark.
        #[derive(Debug, Clone, PartialEq)]
        pub struct Tokens {
            $(pub $c: Rgba,)+
            $(pub $n: f32,)+
            $(pub $t: String,)+
            $(pub $f: bool,)+
            /// Values under names this table lacks: a pane's own tokens, read by name.
            pub extra: BTreeMap<String, ThemeValue>,
        }

        impl Default for Tokens {
            fn default() -> Self {
                Self {
                    $($c: $cv,)+
                    $($n: $nv,)+
                    $($t: $tv.to_owned(),)+
                    $($f: $fv,)+
                    extra: BTreeMap::new(),
                }
            }
        }

        impl Tokens {
            /// Every token's name as a theme file and the theme editor write it.
            pub const NAMES: &'static [&'static str] = &[$($cn,)+ $($nn,)+ $($tn,)+ $($fl,)+];

            /// Sets one token by name. A name this table lacks is kept in `extra`.
            pub fn set(&mut self, name: &str, value: &ThemeValue) -> Result<(), TokenError> {
                match token_name(name) {
                    $($cn => self.$c = value.colour()?,)+
                    $($nn => self.$n = value.number()?,)+
                    $($tn => self.$t = value.text()?,)+
                    $($fl => self.$f = value.flag()?,)+
                    name => {
                        self.extra.insert(name.to_owned(), value.clone());
                    }
                }
                Ok(())
            }

            /// Every token with its value, in the order of [`Tokens::NAMES`].
            pub fn values(&self) -> Vec<(&'static str, ThemeValue)> {
                vec![
                    $(($cn, ThemeValue::Text(self.$c.text())),)+
                    $(($nn, ThemeValue::Number(f64::from(self.$n))),)+
                    $(($tn, ThemeValue::Text(self.$t.clone())),)+
                    $(($fl, ThemeValue::Flag(self.$f)),)+
                ]
            }
        }
    };
}

tokens! {
    colours {
        accent "accent" = rgb(0x8f, 0xb4, 0xff),
        need "needs_you" = rgb(0x78, 0xd2, 0xc6),
        ok "ok" = rgb(0x7f, 0xcf, 0x9a),
        bad "bad" = rgb(0xf5, 0x85, 0x85),
        warn "warn" = rgb(0xe8, 0xa6, 0xbd),
        model "model" = rgb(0xb7, 0x9c, 0xf2),
        context "context" = rgb(0xa6, 0xc4, 0xff),
        fg "text.1" = rgb(0xdc, 0xdf, 0xe6),
        dim "text.2" = rgb(0xb3, 0xb7, 0xc2),
        faint "text.3" = rgb(0x8c, 0x90, 0x9b),
        selected "fill.selected" = rgba(255, 255, 255, 0.11),
        raised "fill.raised" = rgba(255, 255, 255, 0.085),
        panel "fill.panel" = rgba(255, 255, 255, 0.045),
        control_line "line.control" = rgba(255, 255, 255, 0.11),
        added "diff.added" = rgba(80, 190, 120, 0.12),
        removed "diff.removed" = rgba(240, 100, 100, 0.12),
        keyword "code.keyword" = rgb(0xe3, 0xa3, 0xbf),
        type_name "code.type" = rgb(0x86, 0xd6, 0xc8),
        string "code.string" = rgb(0xa9, 0xd9, 0xa0),
        number "code.number" = rgb(0xc6, 0xb3, 0xf7),
        comment "code.comment" = rgb(0x7a, 0x81, 0x96),
        window "window" = rgb(0x14, 0x16, 0x1d),
        window_tint "glass.window.tint" = rgb(17, 19, 23),
        pane_tint "glass.pane.tint" = rgb(8, 9, 12),
        meter "meter" = rgb(0x9a, 0xa3, 0xb5),
        idle "dot.idle" = rgb(0x85, 0x88, 0x91),
        stopped "dot.stopped" = rgb(0x5d, 0x63, 0x72),
    }
    numbers {
        glass_window "glass.window" = 0.2,
        glass_pane "glass.pane" = 0.2,
        glass_pane_focus "glass.pane_in_use" = 0.3,
        lines "lines" = 0.07,
        inner_lines "lines.inner" = 0.045,
        size_title "size.title" = 12.5,
        size_body "size.body" = 11.5,
        size_label "size.label" = 10.5,
        size_meta "size.meta" = 9.5,
        gutter "space.gutter" = 8.0,
        row "space.row" = 20.0,
        line "space.line" = 16.0,
        bar "space.bar" = 22.0,
    }
    texts {
        text_font "font.text" = "system",
        code_font "font.code" = "Geist Mono",
    }
    flags {
        glass "glass" = true,
    }
}

impl Tokens {
    /// A pane's own colour token: the theme's value under `name`, else `default`.
    pub fn colour_or(&self, name: &str, default: Rgba) -> Rgba {
        self.extra
            .get(name)
            .and_then(|value| value.colour().ok())
            .unwrap_or(default)
    }

    /// A pane's own size or strength token: the theme's value under `name`, else `default`.
    pub fn number_or(&self, name: &str, default: f32) -> f32 {
        self.extra
            .get(name)
            .and_then(|value| value.number().ok())
            .unwrap_or(default)
    }

    /// The base with `file`'s values set; a value that cannot be taken is returned by name.
    pub fn themed(file: &ThemeFile) -> (Tokens, Vec<(String, TokenError)>) {
        let mut tokens = Tokens::default();
        let refused = file
            .values
            .iter()
            .filter_map(|(name, value)| Some((name.clone(), tokens.set(name, value).err()?)))
            .collect();
        (tokens, refused)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;
    use crate::model::Model;
    use crate::msg::{Msg, step};

    /// The mockup's own theme file, which names the glass strengths `surface.*`.
    const INK: &str = include_str!("../../../fixtures/mockup/themes/ink.json");

    #[test]
    fn the_mockups_surface_names_set_the_glass_strengths() -> Result<(), Box<dyn Error>> {
        let (tokens, refused) = Tokens::themed(&ThemeFile::parse(INK)?);
        assert_eq!(refused, []);
        let glass = (
            tokens.glass_pane,
            tokens.glass_pane_focus,
            tokens.glass_window,
        );
        assert_eq!(glass, (0.34, 0.46, 0.3));
        assert!(
            !tokens.extra.keys().any(|name| name.starts_with("surface.")),
            "an alias is no token of its own: {:?}",
            tokens.extra
        );
        Ok(())
    }

    #[test]
    fn a_value_is_written_back_under_the_name_it_was_read_by() -> Result<(), Box<dyn Error>> {
        let mut file = ThemeFile::parse(INK)?;
        file.set("glass.pane", ThemeValue::Number(0.5));
        let again = ThemeFile::parse(&file.to_text()?)?;
        assert_eq!(
            again.values.get("surface.pane"),
            Some(&ThemeValue::Number(0.5))
        );
        assert_eq!(again.values.get("glass.pane"), None, "one name, not two");
        assert_eq!(Tokens::themed(&again).0.glass_pane, 0.5);
        Ok(())
    }

    #[test]
    fn a_key_this_app_does_not_know_survives_a_write() -> Result<(), Box<dyn Error>> {
        let text = r#"{"id": "sea", "name": "Sea", "dark": true, "author": "me", "values": {}}"#;
        let again = ThemeFile::parse(text)?.to_text()?;
        assert!(again.contains(r#""author": "me""#), "{again}");
        assert_eq!(ThemeFile::parse(&again)?, ThemeFile::parse(text)?);
        Ok(())
    }

    #[test]
    fn a_theme_is_put_in_use_by_a_message() -> Result<(), Box<dyn Error>> {
        let mut model = Model::default();
        let (ink, _) = Tokens::themed(&ThemeFile::parse(INK)?);
        assert_eq!(step(&mut model, Msg::Theme(Box::new(ink.clone()))), []);
        assert_eq!(model.tokens, ink);
        Ok(())
    }
}
