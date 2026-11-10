//! An omp-shaped theme file overlaid on [`Theme::new`](crate::colors::Theme::new).
//! Colors this UI does not paint are ignored. A missing or unusable file keeps the built-in palette.

use std::path::Path;

use ratatui::style::Color;
use serde_json::{Map, Value};

use crate::colors::{ColorTier, Theme};

pub struct Loaded {
    pub theme: Theme,
    pub warning: Option<String>,
}

/// `file` is `~/.yi/themes/<name>.json`. Absent, the built-in palette is the whole answer.
pub fn load(tier: ColorTier, dark: bool, file: Option<&Path>) -> Loaded {
    let theme = Theme::new(tier, dark);
    let Some(file) = file else {
        return Loaded {
            theme,
            warning: None,
        };
    };
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return kept(theme, format!("theme file {} is missing", file.display()));
        }
        Err(_) => {
            return kept(
                theme,
                format!("theme file {} is unreadable", file.display()),
            );
        }
    };
    if tier == ColorTier::Ansi16 {
        return kept(
            theme,
            format!(
                "theme file {}: a 16-color terminal keeps the built-in palette",
                file.display()
            ),
        );
    }
    let mut painted = Theme::new(tier, dark);
    match overlay(&mut painted, &text) {
        Ok(()) => Loaded {
            theme: painted,
            warning: None,
        },
        Err(reason) => kept(theme, format!("theme file {}: {reason}", file.display())),
    }
}

fn kept(theme: Theme, warning: String) -> Loaded {
    Loaded {
        theme,
        warning: Some(warning),
    }
}

fn overlay(theme: &mut Theme, text: &str) -> Result<(), String> {
    let value: Value = serde_json::from_str(text).map_err(|_| "not a theme file".to_owned())?;
    let colors = value
        .get("colors")
        .and_then(Value::as_object)
        .ok_or_else(|| "not a theme file".to_owned())?;
    let empty = Map::new();
    let vars = value
        .get("vars")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    for (key, raw) in colors {
        if !known(key) {
            continue;
        }
        let color =
            resolve(raw, vars, &mut Vec::new()).map_err(|_| format!("{key} is not a color"))?;
        paint(theme, key, color);
    }
    Ok(())
}

fn known(key: &str) -> bool {
    matches!(
        key,
        "accent"
            | "text"
            | "muted"
            | "dim"
            | "error"
            | "warning"
            | "success"
            | "userMessageBg"
            | "selectedBg"
            | "syntaxKeyword"
            | "syntaxString"
            | "syntaxNumber"
            | "syntaxType"
            | "syntaxFunction"
            | "syntaxVariable"
            | "toolDiffAdded"
            | "toolDiffRemoved"
    )
}

fn paint(theme: &mut Theme, key: &str, color: Color) {
    match key {
        "accent" => theme.accent = color,
        "text" => theme.text = color,
        "muted" => theme.muted = color,
        "dim" => theme.dim = color,
        "error" => theme.error = color,
        "warning" => theme.warning = color,
        "success" => theme.success = color,
        "userMessageBg" => theme.user_bg = Some(color),
        "selectedBg" => theme.selected = Some(color),
        "syntaxKeyword" => theme.syntax_keyword = Some(color),
        "syntaxString" => theme.syntax_string = Some(color),
        "syntaxNumber" => theme.syntax_number = Some(color),
        "syntaxType" => theme.syntax_type = Some(color),
        "syntaxFunction" => theme.syntax_function = Some(color),
        "syntaxVariable" => theme.syntax_variable = Some(color),
        "toolDiffAdded" => theme.diff_added = Some(color),
        "toolDiffRemoved" => theme.diff_removed = Some(color),
        _ => {}
    }
}

fn resolve(value: &Value, vars: &Map<String, Value>, stack: &mut Vec<String>) -> Result<Color, ()> {
    if let Some(index) = value.as_u64() {
        return u8::try_from(index).map(Color::Indexed).map_err(|_| ());
    }
    let Some(text) = value.as_str() else {
        return Err(());
    };
    if text.is_empty() {
        return Ok(Color::Reset);
    }
    if let Some(hex) = text.strip_prefix('#') {
        return parse_hex(hex);
    }
    let name = text.strip_prefix('$').unwrap_or(text);
    if stack.iter().any(|seen| seen == name) {
        return Err(());
    }
    let next = vars.get(name).ok_or(())?;
    stack.push(name.to_owned());
    resolve(next, vars, stack)
}

fn parse_hex(hex: &str) -> Result<Color, ()> {
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(());
    }
    let mut rgb = [0u8; 3];
    for (index, slot) in rgb.iter_mut().enumerate() {
        let pair = hex.get(index * 2..index * 2 + 2).ok_or(())?;
        *slot = u8::from_str_radix(pair, 16).map_err(|_| ())?;
    }
    Ok(Color::Rgb(rgb[0], rgb[1], rgb[2]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colors::DiffRowKind;
    use crate::highlight::Token;

    fn file(name: &str, body: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("yi-theme-{name}-{}.json", std::process::id()));
        std::fs::write(&path, body).unwrap();
        path
    }

    const FIXTURE: &str = r##"{
        "name": "fixture",
        "colors": {
            "accent": "#82aaff",
            "muted": "#112233",
            "selectedBg": "#444444",
            "syntaxKeyword": "#555555",
            "toolDiffAdded": "#00aa00",
            "ignored": "nope"
        }
    }"##;

    #[test]
    fn a_hex_accent_replaces_the_built_in() {
        let path = file("hex", FIXTURE);
        let loaded = load(ColorTier::TrueColor, true, Some(&path));
        let _ = std::fs::remove_file(&path);
        assert!(loaded.warning.is_none());
        assert_eq!(loaded.theme.accent, Color::Rgb(0x82, 0xaa, 0xff));
        assert_eq!(loaded.theme.muted, Color::Rgb(0x11, 0x22, 0x33));
        assert_eq!(loaded.theme.selection_bg(), Color::Rgb(0x44, 0x44, 0x44));
        assert_eq!(
            loaded.theme.syntax_style(Token::Keyword).fg,
            Some(Color::Rgb(0x55, 0x55, 0x55))
        );
        assert_eq!(
            loaded.theme.diff_row(DiffRowKind::Added).fill,
            Some(Color::Rgb(0x00, 0xaa, 0x00))
        );
    }

    #[test]
    fn a_vars_reference_resolves() {
        let path = file(
            "vars",
            r##"{"name":"vars","vars":{"ink":"#010203"},"colors":{"accent":"$ink"}}"##,
        );
        let loaded = load(ColorTier::TrueColor, true, Some(&path));
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded.theme.accent, Color::Rgb(0x01, 0x02, 0x03));
        assert!(loaded.warning.is_none());
    }

    #[test]
    fn a_missing_theme_file_keeps_the_built_in_accent() {
        let path =
            std::env::temp_dir().join(format!("yi-theme-missing-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let loaded = load(ColorTier::TrueColor, true, Some(&path));
        assert_eq!(
            loaded.theme.accent,
            Theme::new(ColorTier::TrueColor, true).accent
        );
        let warning = loaded.warning.expect("a warning");
        assert!(warning.contains("missing"), "{warning}");
    }

    #[test]
    fn a_color_that_is_not_a_value_keeps_the_built_in_accent() {
        let path = file("bad", r#"{"name":"bad","colors":{"accent":"nope"}}"#);
        let loaded = load(ColorTier::TrueColor, true, Some(&path));
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            loaded.theme.accent,
            Theme::new(ColorTier::TrueColor, true).accent
        );
        let warning = loaded.warning.expect("a warning");
        assert!(warning.contains("not a color"), "{warning}");
    }

    #[test]
    fn a_16_color_terminal_keeps_the_named_accent() {
        let path = file("ansi", FIXTURE);
        let loaded = load(ColorTier::Ansi16, true, Some(&path));
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded.theme.accent, Color::Cyan);
        assert!(loaded.warning.is_some());
    }
}
