//! An omp-shaped theme file overlaid on [`Theme::new`](crate::colors::Theme::new).
//! Colors this UI does not paint are ignored. A missing or unusable file keeps the built-in palette.

use std::path::Path;

use ratatui::style::Color;
use serde_json::{Map, Value};

use crate::colors::{ColorTier, Over, Theme};

pub struct Loaded {
    pub theme: Theme,
    pub warning: Option<String>,
}

/// The choice that keeps [`Theme::new`](crate::colors::Theme::new).
pub const BUILT_IN: &str = "built-in";

/// A theme file name is one path segment: letters, digits, `-`, `_`, and `.`.
pub fn theme_name_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[path = "bundled_themes.rs"]
mod bundled_themes;
use bundled_themes::BUNDLED;

/// `built-in`, then the themes Yi ships, then every `*.json` stem in `dir`.
pub fn list(dir: &Path) -> Vec<String> {
    let mut names = std::collections::BTreeSet::from([BUILT_IN.to_owned()]);
    names.extend(BUNDLED.iter().map(|(name, _)| (*name).to_owned()));
    if let Ok(read) = std::fs::read_dir(dir) {
        for entry in read.flatten() {
            let Some(file) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(stem) = file.strip_suffix(".json") else {
                continue;
            };
            if theme_name_ok(stem) {
                names.insert(stem.to_owned());
            }
        }
    }
    let mut names: Vec<String> = names.into_iter().collect();
    if let Some(at) = names.iter().position(|name| name == BUILT_IN) {
        names.swap(0, at);
    }
    names
}

fn bundled(name: &str) -> Option<&'static str> {
    BUNDLED
        .iter()
        .find(|(bundled, _)| *bundled == name)
        .map(|(_, text)| *text)
}

/// `built-in` is the palette with no file. A file in `dir` wins; otherwise a shipped theme.
pub fn preview(tier: ColorTier, dark: bool, dir: &Path, name: &str) -> Loaded {
    if name == BUILT_IN || !theme_name_ok(name) {
        return load(tier, dark, None);
    }
    let file = dir.join(format!("{name}.json"));
    if file.is_file() {
        return load(tier, dark, Some(&file));
    }
    if let Some(text) = bundled(name) {
        return paint_text(tier, dark, name, text);
    }
    load(tier, dark, Some(&file))
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
            if let Some(name) = file.file_stem().and_then(|stem| stem.to_str())
                && theme_name_ok(name)
                && let Some(text) = bundled(name)
            {
                return paint_text(tier, dark, name, text);
            }
            return kept(theme, format!("theme file {} is missing", file.display()));
        }
        Err(_) => {
            return kept(
                theme,
                format!("theme file {} is unreadable", file.display()),
            );
        }
    };
    paint_text(tier, dark, &file.display().to_string(), &text)
}

fn paint_text(tier: ColorTier, dark: bool, label: &str, text: &str) -> Loaded {
    let theme = Theme::new(tier, dark);
    if tier == ColorTier::Ansi16 {
        return kept(
            theme,
            format!("theme file {label}: a 16-color terminal keeps the built-in palette"),
        );
    }
    let mut painted = Theme::new(tier, dark);
    match overlay(&mut painted, text) {
        Ok(()) => Loaded {
            theme: painted,
            warning: None,
        },
        Err(reason) => kept(theme, format!("theme file {label}: {reason}")),
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
        if let Some(slot) = over_slot(key) {
            theme.over[slot as usize] = Some(color);
            continue;
        }
        paint(theme, key, color);
    }
    Ok(())
}

fn known(key: &str) -> bool {
    over_slot(key).is_some()
        || matches!(
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

fn over_slot(key: &str) -> Option<Over> {
    Some(match key {
        "mdHeading" => Over::MdHeading,
        "mdLink" => Over::MdLink,
        "mdLinkUrl" => Over::MdLinkUrl,
        "mdCode" => Over::MdCode,
        "mdCodeBlock" | "mdCodeBlockBorder" => Over::MdCodeBorder,
        "mdListBullet" => Over::MdBullet,
        "mdQuote" | "mdQuoteBorder" => Over::MdQuote,
        "mdHr" => Over::MdHr,
        "userMessageText" => Over::UserText,
        "thinkingText" => Over::Thinking,
        "statusLineModel" => Over::StatusModel,
        "statusLinePath" => Over::StatusPath,
        "statusLineGitClean" => Over::StatusGitClean,
        "statusLineGitDirty" => Over::StatusGitDirty,
        "statusLineSep" => Over::StatusSep,
        "statusLineSpend" | "statusLineCost" => Over::StatusSpend,
        "statusLineContext" => Over::StatusContext,
        "statusLineSubagents" => Over::StatusSubagents,
        "syntaxComment" => Over::SyntaxComment,
        _ => return None,
    })
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
    // `#RRGGBBAA` is the same red, green, and blue. A terminal has no alpha, so the last pair is dropped.
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
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
    fn a_themes_dir_lists_built_in_and_the_file_stem() {
        let dir = std::env::temp_dir().join(format!("yi-theme-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("acid.json"), "{}").unwrap();
        std::fs::write(dir.join("..sneak.json"), "{}").unwrap();
        let names = list(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(names.first().map(String::as_str), Some("built-in"));
        assert!(names.iter().any(|name| name == "acid"));
        assert!(names.iter().any(|name| name == "ember"));
        assert!(names.iter().any(|name| name == "dark"));
        assert!(names.iter().any(|name| name == "titanium"));
        assert!(names.iter().all(|name| !name.contains("sneak")));
    }

    #[test]
    fn a_theme_file_paints_a_heading_the_next_frame_can_read() {
        let path = file(
            "roles",
            r##"{"name":"roles","colors":{"mdHeading":"#010203","statusLinePath":"#040506"}}"##,
        );
        let loaded = load(ColorTier::TrueColor, true, Some(&path));
        let _ = std::fs::remove_file(&path);
        assert!(loaded.warning.is_none(), "{:?}", loaded.warning);
        assert_eq!(
            loaded.theme.pick(Over::MdHeading, Color::Reset),
            Color::Rgb(1, 2, 3)
        );
        assert_eq!(
            loaded.theme.pick(Over::StatusPath, Color::Reset),
            Color::Rgb(4, 5, 6)
        );
        let lines = crate::markdown::render("# Title", 40, &loaded.theme);
        let heading = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.contains("Title"));
        assert_eq!(
            heading.and_then(|span| span.style.fg),
            Some(Color::Rgb(1, 2, 3))
        );
    }

    #[test]
    fn an_eight_digit_hex_keeps_the_red_green_and_blue() {
        let dir = std::env::temp_dir().join(format!("yi-theme-alpha-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let loaded = preview(ColorTier::TrueColor, false, &dir, "light-poimandres");
        assert!(loaded.warning.is_none(), "{:?}", loaded.warning);
        assert_eq!(loaded.theme.accent, Color::Rgb(0x00, 0xce, 0xd1));
        assert_eq!(loaded.theme.selected, Some(Color::Rgb(0x71, 0x7c, 0xb4)));
    }

    #[test]
    fn a_shipped_omp_theme_paints_its_accent() {
        let dir = std::env::temp_dir().join(format!("yi-theme-omp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let loaded = preview(ColorTier::TrueColor, true, &dir, "dark");
        assert_eq!(loaded.theme.accent, Color::Rgb(0xfe, 0xbc, 0x38));
        assert!(loaded.warning.is_none());
    }

    #[test]
    fn a_preview_paints_the_named_file_and_built_in_restores_the_palette() {
        let dir = std::env::temp_dir().join(format!("yi-theme-preview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("acid.json"),
            r##"{"name":"acid","colors":{"accent":"#010203"}}"##,
        )
        .unwrap();
        let painted = preview(ColorTier::TrueColor, true, &dir, "acid");
        let restored = preview(ColorTier::TrueColor, true, &dir, BUILT_IN);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(painted.theme.accent, Color::Rgb(0x01, 0x02, 0x03));
        assert!(painted.warning.is_none());
        assert_eq!(
            restored.theme.accent,
            Theme::new(ColorTier::TrueColor, true).accent
        );
    }

    #[test]
    fn a_16_color_preview_keeps_the_named_accent() {
        let dir = std::env::temp_dir().join(format!("yi-theme-ansi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("acid.json"),
            r##"{"name":"acid","colors":{"accent":"#010203"}}"##,
        )
        .unwrap();
        let loaded = preview(ColorTier::Ansi16, true, &dir, "acid");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(loaded.theme.accent, Color::Cyan);
        assert!(loaded.warning.is_some());
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
