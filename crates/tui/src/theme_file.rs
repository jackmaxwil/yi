//! An omp-shaped theme laid over [`Theme::new`](crate::colors::Theme::new): its `vars`, then the
//! `colors` keys in [`crate::colors::THEME_KEYS`]; any other key is ignored.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ratatui::style::Color;
use serde_json::{Map, Value};

use crate::app::{App, Bottom};
use crate::colors::{ColorTier, THEME_KEYS, Theme};
use crate::popup::ListPopup;

pub const BUILT_IN: &str = "built-in";
/// Incident: an unbounded `$var` chain recursed once a hop and overflowed the stack at startup.
const HOPS: usize = 16;
/// A shipped theme is under 4 KB; a FIFO or `/dev/zero` named `x.json` would never end.
const MAX_BYTES: u64 = 256 * 1024;
const PACKED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/themes.zz"));

pub struct Loaded {
    pub theme: Theme,
    pub warning: Option<String>,
}

/// A theme name is one path segment: letters, digits, `-`, `_`, and `.`.
pub fn theme_name_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn themes_dir(home: Option<&Path>) -> Option<PathBuf> {
    Some(home?.join(".yi").join("themes"))
}

pub fn ground_id(dark: bool) -> &'static str {
    if dark { "theme.dark" } else { "theme.light" }
}

pub fn is_theme_row(id: &str) -> bool {
    id == ground_id(true) || id == ground_id(false)
}

fn bundled() -> &'static Map<String, Value> {
    static THEMES: OnceLock<Map<String, Value>> = OnceLock::new();
    THEMES.get_or_init(|| {
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib(PACKED).unwrap_or_default();
        serde_json::from_slice(&raw).unwrap_or_default()
    })
}

/// `built-in`, then the shipped themes and every `*.json` stem in `dir`, sorted.
pub fn list(dir: Option<&Path>) -> Vec<String> {
    let mut names: std::collections::BTreeSet<String> = bundled().keys().cloned().collect();
    let entries = dir.and_then(|dir| std::fs::read_dir(dir).ok());
    for entry in entries.into_iter().flatten().flatten() {
        let file = entry.file_name();
        if let Some(stem) = file.to_str().and_then(|file| file.strip_suffix(".json"))
            && theme_name_ok(stem)
        {
            names.insert(stem.to_owned());
        }
    }
    std::iter::once(BUILT_IN.to_owned()).chain(names).collect()
}

/// `name` from `dir`, else the shipped theme of that name; `built-in` is the palette alone.
pub fn preview(tier: ColorTier, dark: bool, dir: Option<&Path>, name: &str) -> Loaded {
    let built = Theme::new(tier, dark);
    let kept = |why: String| Loaded {
        theme: built,
        warning: Some(why),
    };
    if name == BUILT_IN {
        return Loaded {
            theme: built,
            warning: None,
        };
    }
    if !theme_name_ok(name) {
        return kept(format!("theme: `{name}` is not a theme name"));
    }
    let file = dir.map(|dir| dir.join(format!("{name}.json")));
    let value = match file.as_deref().map(read).transpose() {
        Ok(Some(Some(value))) => value,
        Ok(_) => match bundled().get(name) {
            Some(value) => value.clone(),
            None => return kept(format!("theme {name}: no such file or shipped theme")),
        },
        Err(why) => return kept(format!("theme {name}: {why}")),
    };
    if tier == ColorTier::Ansi16 {
        return kept(format!(
            "theme {name}: a 16-color terminal keeps its colors"
        ));
    }
    let mut theme = built;
    match overlay(&mut theme, &value) {
        Ok(()) => Loaded {
            theme,
            warning: None,
        },
        Err(why) => kept(format!("theme {name}: {why}")),
    }
}

/// The palette a launch paints: tier and ground from the environment, any warning on stderr.
pub fn startup(config: Option<&yi_types::config::ThemeConfig>) -> Theme {
    let env = |key| std::env::var(key).ok();
    let tier = crate::colors::detect_tier(env("COLORTERM").as_deref(), env("TERM").as_deref());
    let dark = crate::colors::detect_dark(env("COLORFGBG").as_deref());
    let name = config.and_then(|config| if dark { &config.dark } else { &config.light }.as_deref());
    let dir = themes_dir(home().as_deref());
    let name = name.map(str::trim).filter(|name| !name.is_empty());
    let loaded = preview(tier, dark, dir.as_deref(), name.unwrap_or(BUILT_IN));
    if let Some(warning) = loaded.warning {
        eprintln!("warning: {warning}");
    }
    loaded.theme
}

/// `None` when the file is absent; any other failure is the reason the palette stays.
fn read(file: &Path) -> Result<Option<Value>, String> {
    let shown = file.display();
    let meta = match std::fs::metadata(file) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(format!("{shown} is unreadable")),
    };
    if !meta.is_file() {
        return Err(format!("{shown} is not a file"));
    }
    let mut text = String::new();
    std::fs::File::open(file)
        .and_then(|open| {
            open.take(MAX_BYTES.saturating_add(1))
                .read_to_string(&mut text)
        })
        .map_err(|_| format!("{shown} is unreadable"))?;
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > MAX_BYTES {
        return Err(format!(
            "{shown} is past the {MAX_BYTES}-byte cap on a theme file"
        ));
    }
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|_| format!("{shown} is not JSON"))
}

fn overlay(theme: &mut Theme, value: &Value) -> Result<(), String> {
    let colors = value
        .get("colors")
        .and_then(Value::as_object)
        .ok_or("not a theme file: no `colors` object")?;
    let vars = value.get("vars").and_then(Value::as_object);
    for (key, slot) in THEME_KEYS {
        if let Some(raw) = colors.get(key) {
            let color = resolve(raw, vars, theme.tier).map_err(|why| format!("{key} {why}"))?;
            theme.set(slot, color);
        }
    }
    Ok(())
}

/// A name is a `vars` entry, followed at most [`HOPS`] times, so a cycle ends as well.
fn resolve<'a>(
    mut value: &'a Value,
    vars: Option<&'a Map<String, Value>>,
    tier: ColorTier,
) -> Result<Color, String> {
    for _ in 0..=HOPS {
        if let Some(index) = value.as_u64() {
            return u8::try_from(index)
                .map(Color::Indexed)
                .map_err(|_| "is past color 255".to_owned());
        }
        let text = value.as_str().ok_or("is not a color")?;
        if text.is_empty() {
            return Ok(Color::Reset);
        }
        if let Some(hex) = text.strip_prefix('#') {
            return parse_hex(hex, tier).ok_or_else(|| format!("`{text}` is not a hex color"));
        }
        let name = text.strip_prefix('$').unwrap_or(text);
        value = vars
            .and_then(|vars| vars.get(name))
            .ok_or_else(|| format!("names `{name}`, which is not in vars"))?;
    }
    Err(format!("passes through more than {HOPS} vars"))
}

/// `#RRGGBBAA` keeps the red, green and blue: a terminal has no alpha.
fn parse_hex(hex: &str, tier: ColorTier) -> Option<Color> {
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(hex.get(at..at.saturating_add(2))?, 16).ok();
    let (red, green, blue) = (byte(0)?, byte(2)?, byte(4)?);
    Some(match tier {
        ColorTier::Ansi256 => Color::Indexed(cube(red, green, blue)),
        ColorTier::TrueColor | ColorTier::Ansi16 => Color::Rgb(red, green, blue),
    })
}

/// The nearest of xterm's 6x6x6 cube, whose levels are 0, 95, 135, 175, 215 and 255.
fn cube(red: u8, green: u8, blue: u8) -> u8 {
    let level = |value: u8| match value {
        0..48 => 0,
        48..115 => 1,
        _ => (value.saturating_sub(35)) / 40,
    };
    let at = level(red)
        .saturating_mul(36)
        .saturating_add(level(green).saturating_mul(6));
    at.saturating_add(level(blue)).saturating_add(16)
}

/// `/theme`: the list, and the palette that any close but a save puts back.
pub(crate) struct Picker {
    pub(crate) list: ListPopup,
    restore: Theme,
    shown: String,
    warned: bool,
}

pub(crate) fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

impl App {
    pub fn set_theme(&mut self, mut theme: Theme) {
        theme.run = self.theme.run;
        theme.rows = self.theme.rows;
        self.theme = theme;
        self.scheduler.request();
    }

    pub(crate) fn open_theme_list(&mut self) {
        let id = ground_id(self.theme.dark);
        let current = home()
            .and_then(|home| yi_runtime::settings::current(&home).ok())
            .and_then(|rows| rows.into_iter().find(|row| row.id == id))
            .and_then(|row| row.value.as_str().map(str::to_owned))
            .unwrap_or_else(|| BUILT_IN.to_owned());
        let mut list = ListPopup::new(' ', list(themes_dir(home().as_deref()).as_deref()));
        list.selected = list
            .items
            .iter()
            .position(|item| *item == current)
            .unwrap_or(0);
        list.note = Some("enter saves · esc restores".to_owned());
        let shown = list.items.get(list.selected).cloned().unwrap_or(current);
        let picker = Picker {
            list,
            restore: self.theme,
            shown,
            warned: false,
        };
        self.bottom = Some(Bottom::Theme(Box::new(picker)));
        self.scheduler.request();
    }

    /// Paints the selection on the live screen only; scrollback keeps its rows until a save.
    pub(crate) fn preview_theme(&mut self, picker: &mut Picker, name: &str) {
        if picker.shown == name {
            return;
        }
        picker.shown = name.to_owned();
        let dir = themes_dir(home().as_deref());
        let loaded = preview(self.theme.tier, self.theme.dark, dir.as_deref(), name);
        if let Some(warning) = loaded.warning.filter(|_| !picker.warned) {
            picker.warned = true;
            self.notice(format!("warning: {warning}"));
        }
        self.set_theme(loaded.theme);
    }

    /// Puts back the palette of an open `/theme` that something else is about to replace.
    pub(crate) fn leave_theme_list(&mut self) {
        if let Some(Bottom::Theme(picker)) = self.bottom.take() {
            self.set_theme(picker.restore);
        }
    }

    pub(crate) fn close_theme_list(&mut self, picker: &Picker) {
        self.set_theme(picker.restore);
    }

    /// Saves `name` for this ground, then rewrites scrollback in it the way a resize does.
    pub(crate) fn commit_theme(&mut self, picker: &mut Picker, name: &str) {
        let value = match name {
            BUILT_IN => Value::Null,
            name => Value::String(name.to_owned()),
        };
        let id = ground_id(self.theme.dark);
        let saved = home()
            .ok_or_else(|| "no home directory, so the choice was not saved".to_owned())
            .and_then(|home| yi_runtime::settings::save(&home, id, value));
        if let Err(why) = saved {
            self.close_theme_list(picker);
            self.notice(format!("theme: {why}"));
            return;
        }
        self.preview_theme(picker, name);
        self.reflow.schedule_immediate(std::time::Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colors::{DiffRowKind, Over};
    use crate::highlight::Token;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn dir(name: &str) -> Result<PathBuf, std::io::Error> {
        let dir = std::env::temp_dir().join(format!("yi-theme-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn load(dir: &Path, name: &str) -> Loaded {
        preview(ColorTier::TrueColor, true, Some(dir), name)
    }

    #[test]
    fn every_shipped_theme_loads_on_both_grounds() {
        let names = list(None);
        assert_eq!(
            names.len(),
            105,
            "built-in and the 104 shipped themes: {names:?}"
        );
        for name in names {
            for dark in [true, false] {
                let loaded = preview(ColorTier::TrueColor, dark, None, &name);
                assert_eq!(loaded.warning, None, "{name} dark={dark}");
            }
        }
    }

    #[test]
    fn a_vars_chain_is_followed_to_the_cap_and_refused_past_it() -> TestResult {
        let dir = dir("hops")?;
        let chain = |hops: usize| {
            let mut vars: Vec<String> = (0..hops)
                .map(|at| format!("\"v{at}\":\"v{}\"", at + 1))
                .collect();
            vars.push(format!("\"v{hops}\":\"#010203\""));
            format!(
                "{{\"vars\":{{{}}},\"colors\":{{\"accent\":\"v0\"}}}}",
                vars.join(",")
            )
        };
        for (name, hops) in [("at", HOPS - 1), ("past", HOPS), ("deep", 200_000)] {
            std::fs::write(dir.join(format!("{name}.json")), chain(hops))?;
        }
        let at = load(&dir, "at");
        let past = load(&dir, "past");
        let deep = load(&dir, "deep");
        std::fs::remove_dir_all(&dir)?;
        assert_eq!((at.theme.accent, at.warning), (Color::Rgb(1, 2, 3), None));
        let refused = Some("theme past: accent passes through more than 16 vars".to_owned());
        assert_eq!(past.warning, refused);
        assert!(
            deep.warning.is_some_and(|why| why.contains("byte cap")),
            "the cap reads it first"
        );
        assert_eq!(
            deep.theme.accent,
            Theme::new(ColorTier::TrueColor, true).accent
        );
        Ok(())
    }

    #[test]
    fn a_theme_file_at_the_byte_cap_loads_and_one_byte_past_it_is_refused() -> TestResult {
        let dir = dir("cap")?;
        let body = r##"{"colors":{"accent":"#010203"}}"##;
        let cap = usize::try_from(MAX_BYTES)?;
        let padded = |len: usize| format!("{body}{}", " ".repeat(len.saturating_sub(body.len())));
        std::fs::write(dir.join("at.json"), padded(cap))?;
        std::fs::write(dir.join("past.json"), padded(cap + 1))?;
        std::fs::create_dir(dir.join("folder.json"))?;
        let at = load(&dir, "at");
        let past = load(&dir, "past");
        let folder = load(&dir, "folder");
        std::fs::remove_dir_all(&dir)?;
        assert_eq!((at.theme.accent, at.warning), (Color::Rgb(1, 2, 3), None));
        let past = past.warning.unwrap_or_default();
        assert!(
            past.ends_with("past.json is past the 262144-byte cap on a theme file"),
            "{past}"
        );
        assert!(
            folder
                .warning
                .is_some_and(|why| why.ends_with("folder.json is not a file"))
        );
        Ok(())
    }

    #[test]
    fn a_file_paints_the_roles_it_names_and_a_bad_color_keeps_the_palette() -> TestResult {
        let dir = dir("roles")?;
        let good = r##"{"vars":{"ink":"#82aaff"},"colors":{"accent":"$ink","selectedBg":"#444444",
            "syntaxKeyword":"#555555","toolDiffAdded":"#00aa00","mdHeading":"#010203","ignored":"x"}}"##;
        std::fs::write(dir.join("good.json"), good)?;
        std::fs::write(dir.join("bad.json"), r#"{"colors":{"accent":"nope"}}"#)?;
        let good = load(&dir, "good");
        let bad = load(&dir, "bad");
        std::fs::remove_dir_all(&dir)?;
        let theme = good.theme;
        assert_eq!(good.warning, None);
        assert_eq!(theme.accent, Color::Rgb(0x82, 0xaa, 0xff));
        assert_eq!(theme.selection_bg(), Color::Rgb(0x44, 0x44, 0x44));
        let keyword = theme.syntax_style(Token::Keyword).fg;
        assert_eq!(keyword, Some(Color::Rgb(0x55, 0x55, 0x55)));
        assert_eq!(
            theme.diff_row(DiffRowKind::Added).fill,
            Some(Color::Rgb(0, 0xaa, 0))
        );
        let lines = crate::markdown::render("# Title", 40, &theme);
        let span = lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content == "Title");
        assert_eq!(
            span.and_then(|span| span.style.fg),
            Some(Color::Rgb(1, 2, 3))
        );
        let why = Some("theme bad: accent names `nope`, which is not in vars".to_owned());
        assert_eq!(bad.warning, why);
        assert_eq!(
            bad.theme.accent,
            Theme::new(ColorTier::TrueColor, true).accent
        );
        Ok(())
    }

    #[test]
    fn a_file_wins_over_the_shipped_theme_and_lists_beside_it() -> TestResult {
        let dir = dir("list")?;
        std::fs::write(
            dir.join("dark.json"),
            r##"{"colors":{"accent":"#010203"}}"##,
        )?;
        std::fs::write(dir.join("..sneak.json"), "{}")?;
        let names = list(Some(&dir));
        let own = load(&dir, "dark");
        std::fs::remove_dir_all(&dir)?;
        assert_eq!(names.first().map(String::as_str), Some(BUILT_IN));
        assert!(
            names.iter().all(|name| !name.contains("sneak")),
            "{names:?}"
        );
        assert_eq!(own.theme.accent, Color::Rgb(1, 2, 3));
        let shipped = preview(ColorTier::TrueColor, true, None, "dark");
        assert_eq!(shipped.theme.accent, Color::Rgb(0xfe, 0xbc, 0x38));
        Ok(())
    }

    #[test]
    fn an_eight_digit_hex_keeps_the_red_green_and_blue() {
        let loaded = preview(ColorTier::TrueColor, false, None, "light-poimandres");
        assert_eq!(loaded.warning, None);
        assert_eq!(loaded.theme.accent, Color::Rgb(0x00, 0xce, 0xd1));
        assert_eq!(
            loaded.theme.from_file(Over::Selected),
            Some(Color::Rgb(0x71, 0x7c, 0xb4))
        );
    }

    #[test]
    fn a_256_color_terminal_gets_the_nearest_cube_color_not_24_bit() {
        let loaded = preview(ColorTier::Ansi256, true, None, "dark-nord");
        assert_eq!(loaded.warning, None);
        // #88c0d0: 136 -> level 2, 192 -> 3, 208 -> 4, so 16 + 72 + 18 + 4.
        assert_eq!(loaded.theme.accent, Color::Indexed(110));
        // toolDiffAdded is nord14, #a3be8c: levels 3, 3 and 2, so 16 + 108 + 18 + 2.
        assert_eq!(
            loaded.theme.diff_row(DiffRowKind::Added).fill,
            Some(Color::Indexed(144))
        );
        assert_eq!(
            (cube(0, 0, 0), cube(255, 255, 255), cube(47, 48, 114)),
            (16, 231, 23)
        );
    }

    #[test]
    fn built_in_and_a_16_color_terminal_keep_the_palette() {
        let built = Theme::new(ColorTier::TrueColor, true);
        let unset = preview(ColorTier::TrueColor, true, None, BUILT_IN);
        assert_eq!(
            (unset.theme.palette(), unset.warning),
            (built.palette(), None)
        );
        let ansi = preview(ColorTier::Ansi16, true, None, "dark");
        assert_eq!(ansi.theme.accent, Color::Cyan);
        let why = Some("theme dark: a 16-color terminal keeps its colors".to_owned());
        assert_eq!(ansi.warning, why);
    }
}
