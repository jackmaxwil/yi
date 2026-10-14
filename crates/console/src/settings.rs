//! The settings pane: the focused session's live options, every config default the daemon
//! serves, and the key table, a tab each.
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use yi_tui::colors::Theme;
use yi_types::acp::AcpConfigOption;
use yi_types::settings::{Applies, SettingKind, SettingRow, TABS, pane_edits_list};

use crate::model::SessionId;

pub const SESSION: usize = 0;
pub const KEYS: usize = TABS.len() + 1;

pub fn tab_name(tab: usize) -> &'static str {
    match tab {
        SESSION => "Session",
        KEYS => "Keys",
        n => TABS.get(n.saturating_sub(1)).copied().unwrap_or("?"),
    }
}

pub enum Edit {
    Pick { options: Vec<String>, cursor: usize },
    Type(String),
}

pub struct Settings {
    pub tab: usize,
    pub cursor: usize,
    pub session: Option<SessionId>,
    /// `None` until the daemon answers `_yi/settings`.
    pub rows: Option<Vec<SettingRow>>,
    pub edit: Option<Edit>,
    pub status: Option<String>,
}

impl Settings {
    pub fn new(session: Option<SessionId>, tab: usize) -> Self {
        Self {
            tab,
            cursor: 0,
            session,
            rows: None,
            edit: None,
            status: None,
        }
    }

    fn tab_rows(&self) -> Vec<&SettingRow> {
        let name = tab_name(self.tab);
        self.rows
            .iter()
            .flatten()
            .filter(|row| row.tab == name)
            .collect()
    }
}

/// What a key asks the app to send; the pane itself only moves its cursor and edit.
pub enum Intent {
    None,
    SaveDefault { id: String, value: Value },
    SetSession { config_id: String, value: String },
    Note(String),
}

/// The config default a session option mirrors, for "make this the default".
fn default_of(config_id: &str) -> Option<&'static str> {
    match config_id {
        "model" => Some("model"),
        "thought_level" => Some("thinking"),
        "mode" => Some("permissions.mode"),
        _ => None,
    }
}

/// `models.primary` wins over `model` when both are set, so a default model goes where it reads.
fn primary_over<'a>(settings: &Settings, id: &'a str) -> &'a str {
    let primary = settings
        .rows
        .iter()
        .flatten()
        .any(|row| row.id == "models.primary" && !row.value.is_null());
    if id == "model" && primary {
        "models.primary"
    } else {
        id
    }
}

fn count(settings: &Settings, options: &[AcpConfigOption]) -> usize {
    match settings.tab {
        SESSION => options.len(),
        KEYS => crate::keys::CHORDS.len(),
        _ => settings.tab_rows().len(),
    }
}

fn typed(kind: &SettingKind, text: &str) -> Result<Value, String> {
    match kind {
        SettingKind::Number { .. } => serde_json::from_str::<serde_json::Number>(text.trim())
            .map(Value::Number)
            .map_err(|_| format!("{text:?} is not a number")),
        SettingKind::List => {
            let items = yi_types::settings::comma_list(text);
            Ok(if items.is_empty() {
                Value::Null
            } else {
                Value::Array(items.into_iter().map(Value::String).collect())
            })
        }
        _ => Ok(Value::String(text.to_owned())),
    }
}

pub fn key(settings: &mut Settings, key: KeyEvent, options: &[AcpConfigOption]) -> Intent {
    let rows = count(settings, options);
    match settings.edit.take() {
        Some(Edit::Pick {
            options: picks,
            cursor,
        }) => {
            let len = picks.len().max(1);
            let (next, chosen) = match key.code {
                KeyCode::Up => (
                    Some(cursor.checked_sub(1).unwrap_or(len.saturating_sub(1))),
                    None,
                ),
                KeyCode::Down => (Some(cursor.saturating_add(1) % len), None),
                KeyCode::Enter => (None, picks.get(cursor).cloned()),
                KeyCode::Esc => (None, None),
                _ => (Some(cursor), None),
            };
            if let Some(cursor) = next {
                settings.edit = Some(Edit::Pick {
                    options: picks,
                    cursor,
                });
            }
            return chosen.map_or(Intent::None, |choice| {
                commit(settings, options, Value::String(choice))
            });
        }
        Some(Edit::Type(mut text)) => {
            match key.code {
                KeyCode::Enter => return commit_typed(settings, options, &text),
                KeyCode::Esc => return Intent::None,
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char(ch) => text.push(ch),
                _ => {}
            }
            settings.edit = Some(Edit::Type(text));
            return Intent::None;
        }
        None => {}
    }
    match key.code {
        KeyCode::Left | KeyCode::Right => {
            let tabs = KEYS.saturating_add(1);
            settings.tab = if key.code == KeyCode::Right {
                settings.tab.saturating_add(1) % tabs
            } else {
                settings.tab.checked_sub(1).unwrap_or(KEYS)
            };
            settings.cursor = 0;
            Intent::None
        }
        KeyCode::Up => {
            settings.cursor = settings.cursor.saturating_sub(1);
            Intent::None
        }
        KeyCode::Down => {
            settings.cursor = settings
                .cursor
                .saturating_add(1)
                .min(rows.saturating_sub(1));
            Intent::None
        }
        KeyCode::Enter => begin(settings, options),
        KeyCode::Char('u') if settings.tab != SESSION && settings.tab != KEYS => {
            match settings
                .tab_rows()
                .get(settings.cursor)
                .map(|row| (&row.kind, row.id.clone()))
            {
                Some((SettingKind::Json, id)) => {
                    Intent::Note(format!("{id} is edited in ~/.yi/config.json"))
                }
                Some((SettingKind::List, id)) if !pane_edits_list(&id) => {
                    Intent::Note(format!("{id} is edited in ~/.yi/config.json"))
                }
                _ => commit(settings, options, Value::Null),
            }
        }
        KeyCode::Char('d') if settings.tab == SESSION => {
            let option = options.get(settings.cursor);
            match option.and_then(|option| Some((default_of(&option.config_id)?, option))) {
                Some((id, option)) => Intent::SaveDefault {
                    id: primary_over(settings, id).to_owned(),
                    value: Value::String(option.current_value.clone()),
                },
                None => Intent::None,
            }
        }
        _ => Intent::None,
    }
}

fn begin(settings: &mut Settings, options: &[AcpConfigOption]) -> Intent {
    if settings.tab == KEYS {
        return Intent::None;
    }
    if settings.tab == SESSION {
        let Some(option) = options.get(settings.cursor) else {
            return Intent::None;
        };
        let picks: Vec<String> = option
            .options
            .iter()
            .map(|choice| choice.value.clone())
            .collect();
        let cursor = picks
            .iter()
            .position(|value| *value == option.current_value)
            .unwrap_or(0);
        settings.edit = Some(Edit::Pick {
            options: picks,
            cursor,
        });
        return Intent::None;
    }
    let Some(row) = settings
        .tab_rows()
        .get(settings.cursor)
        .map(|row| (*row).clone())
    else {
        return Intent::None;
    };
    match &row.kind {
        SettingKind::Toggle => {
            let on = if row.value.is_null() {
                &row.default
            } else {
                &row.value
            };
            commit(settings, options, Value::Bool(on != &Value::Bool(true)))
        }
        SettingKind::Choice { options: picks } => {
            let current = row.value.as_str().unwrap_or_default();
            let cursor = picks.iter().position(|pick| pick == current).unwrap_or(0);
            settings.edit = Some(Edit::Pick {
                options: picks.clone(),
                cursor,
            });
            Intent::None
        }
        SettingKind::Number { .. } | SettingKind::Text | SettingKind::Model => {
            let text = match &row.value {
                Value::String(text) => text.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            };
            settings.edit = Some(Edit::Type(text));
            Intent::None
        }
        SettingKind::List if pane_edits_list(&row.id) && row.value.is_null() => {
            settings.edit = Some(Edit::Type(String::new()));
            Intent::None
        }
        SettingKind::List
            if pane_edits_list(&row.id)
                && row
                    .value
                    .as_array()
                    .is_some_and(|items| items.iter().all(Value::is_string)) =>
        {
            settings.edit = Some(Edit::Type(shown_value(&row.value)));
            Intent::None
        }
        SettingKind::List | SettingKind::Json => {
            Intent::Note(format!("{} is edited in ~/.yi/config.json", row.id))
        }
    }
}

fn commit(settings: &Settings, options: &[AcpConfigOption], value: Value) -> Intent {
    if settings.tab == SESSION {
        let Some(option) = options.get(settings.cursor) else {
            return Intent::None;
        };
        return match value {
            Value::String(value) => Intent::SetSession {
                config_id: option.config_id.clone(),
                value,
            },
            _ => Intent::None,
        };
    }
    match settings.tab_rows().get(settings.cursor) {
        Some(row) => Intent::SaveDefault {
            id: row.id.clone(),
            value,
        },
        None => Intent::None,
    }
}

fn commit_typed(settings: &Settings, options: &[AcpConfigOption], text: &str) -> Intent {
    let Some(row) = settings
        .tab_rows()
        .get(settings.cursor)
        .map(|row| (*row).clone())
    else {
        return Intent::None;
    };
    if text.trim().is_empty() {
        return commit(settings, options, Value::Null);
    }
    match typed(&row.kind, text) {
        Ok(value) => commit(settings, options, value),
        Err(why) => Intent::Note(why),
    }
}

fn shown(row: &SettingRow) -> String {
    let entries = match &row.kind {
        SettingKind::Json => true,
        SettingKind::List => !pane_edits_list(&row.id),
        _ => false,
    };
    match (&row.value, &row.default) {
        (Value::Number(count), _) if entries => format!("{count} entries"),
        (Value::Null, Value::Bool(true)) => "on (default)".to_owned(),
        (Value::Null, Value::Bool(false)) => "off (default)".to_owned(),
        (value, _) => shown_value(value),
    }
}

fn shown_value(value: &Value) -> String {
    match value {
        Value::Null => "unset".to_owned(),
        Value::Bool(true) => "on".to_owned(),
        Value::Bool(false) => "off".to_owned(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map_or_else(|| item.to_string(), str::to_owned)
            })
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}

fn applies(applies: Applies) -> &'static str {
    match applies {
        Applies::Live => "now",
        Applies::NextSession => "next session",
        Applies::DaemonRestart => "after restart",
        Applies::ConsoleRestart => "next console",
        Applies::SoloTui => "solo tui only",
    }
}

fn clip(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return format!("{text:<width$}");
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn tab_bar(settings: &Settings, width: usize, theme: &Theme) -> Line<'static> {
    let span = |first: usize, last: usize| -> usize {
        let names: usize = (first..=last).map(|i| tab_name(i).chars().count()).sum();
        names + (last - first) + 6
    };
    let (mut first, mut last) = (settings.tab, settings.tab);
    loop {
        let right = (last < KEYS && span(first, last + 1) <= width).then(|| last + 1);
        last = right.unwrap_or(last);
        let left = first.checked_sub(1).filter(|lo| span(*lo, last) <= width);
        first = left.unwrap_or(first);
        if right.is_none() && left.is_none() {
            break;
        }
    }
    let dim = theme.dim_style();
    let mut spans = vec![Span::styled(if first > 0 { "‹ " } else { "  " }, dim)];
    for index in first..=last {
        if index > first {
            spans.push(Span::raw(" "));
        }
        spans.push(if index == settings.tab {
            let chip = Modifier::BOLD | Modifier::REVERSED;
            let name = format!(" {} ", tab_name(index));
            Span::styled(name, theme.accent_style().add_modifier(chip))
        } else {
            Span::styled(tab_name(index), dim)
        });
    }
    spans.push(Span::styled(if last < KEYS { " ›" } else { "" }, dim));
    Line::from(spans)
}

/// (label, value, when it applies, help) of every row on the current tab.
type Shown = (String, String, String, String);

fn shown_rows(settings: &Settings, options: &[AcpConfigOption], cmd: bool) -> Vec<Shown> {
    match settings.tab {
        SESSION => options
            .iter()
            .map(|option| {
                let named = option
                    .options
                    .iter()
                    .find(|choice| choice.value == option.current_value)
                    .map_or(option.current_value.clone(), |choice| choice.name.clone());
                let help = default_of(&option.config_id).map_or_else(String::new, |id| {
                    format!("this session only; d saves it as the default {id}")
                });
                (option.name.clone(), named, "now".to_owned(), help)
            })
            .collect(),
        KEYS => crate::keys::CHORDS
            .iter()
            .map(|chord| {
                let prefix = if chord.prefix.is_empty() {
                    String::new()
                } else {
                    format!("ctrl+b {}", chord.prefix)
                };
                (
                    chord.what.to_owned(),
                    chord.key(cmd).to_owned(),
                    prefix,
                    String::new(),
                )
            })
            .collect(),
        _ => settings
            .tab_rows()
            .into_iter()
            .map(|row| {
                (
                    row.label.clone(),
                    shown(row),
                    applies(row.applies).to_owned(),
                    format!(
                        "{} · {} · applies {}",
                        row.id,
                        row.help,
                        applies(row.applies)
                    ),
                )
            })
            .collect(),
    }
}

/// The pane's lines; `options` is the session's, `visible` its text rows.
pub fn view(
    settings: &Settings,
    options: &[AcpConfigOption],
    cmd: bool,
    width: usize,
    visible: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = vec![tab_bar(settings, width, theme), Line::default()];
    let rows = shown_rows(settings, options, cmd);
    let waiting = settings.tab != SESSION && settings.tab != KEYS && settings.rows.is_none();
    if waiting {
        lines.push(Line::styled(
            " asking the daemon for the config…",
            theme.dim_style(),
        ));
    } else if rows.is_empty() {
        let empty = if settings.tab == SESSION {
            " no session in this pane"
        } else {
            " nothing here"
        };
        lines.push(Line::styled(empty, theme.dim_style()));
    }
    let tail = if width >= 60 { 14 } else { 0 };
    let value = if settings.tab == KEYS {
        14
    } else {
        width.saturating_sub(4) / 3
    };
    let label = width.saturating_sub(value.saturating_add(tail).saturating_add(1));
    let room = visible.saturating_sub(8).max(1);
    let first = settings.cursor.saturating_sub(room.saturating_sub(1));
    for (index, (name, shown, applies, _)) in rows.iter().enumerate().skip(first).take(room) {
        let chosen = index == settings.cursor;
        let marker = if chosen { "▸ " } else { "  " };
        let editing = chosen.then_some(settings.edit.as_ref()).flatten();
        let shown = match editing {
            Some(Edit::Type(text)) => format!("{text}▌"),
            Some(Edit::Pick { options, cursor }) => options
                .get(*cursor)
                .map_or_else(String::new, |pick| format!("‹ {pick} ›")),
            None => shown.clone(),
        };
        let style = if chosen {
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {marker}{}", clip(name, label.saturating_sub(3))),
                style,
            ),
            Span::styled(clip(&shown, value), theme.accent_style()),
            Span::styled(clip(applies, tail), theme.dim_style()),
        ]));
    }
    if rows.len() > room {
        lines.push(Line::styled(
            format!(
                " […] rows {}-{} of {}",
                first.saturating_add(1),
                first.saturating_add(room).min(rows.len()),
                rows.len()
            ),
            theme.dim_style(),
        ));
    }
    lines.push(Line::default());
    let help = rows
        .get(settings.cursor)
        .map_or("", |(_, _, _, help)| help.as_str());
    let help = Line::styled(format!(" {help}"), theme.muted_style());
    lines.extend(
        yi_tui::wrap::wrap_line(&help, width.saturating_sub(1), " ")
            .into_iter()
            .take(3),
    );
    if let Some(status) = &settings.status {
        lines.push(Line::styled(format!(" {status}"), theme.accent_style()));
    }
    let hint = match (&settings.edit, settings.tab) {
        (Some(Edit::Type(_)), _) => " type · enter save · empty unsets · esc cancel",
        (Some(Edit::Pick { .. }), _) => " ↑↓ choose · enter save · esc cancel",
        (None, SESSION) => " ↑↓ move · ←→ tab · enter change · d make default",
        (None, KEYS) => " ↑↓ move · ←→ tab",
        (None, _) => " ↑↓ move · ←→ tab · enter change · u unset",
    };
    lines.push(Line::styled(hint, theme.dim_style()));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use yi_tui::colors::ColorTier;

    fn strip(tab: usize, width: usize) -> String {
        let theme = Theme::new(ColorTier::TrueColor, true);
        let line = tab_bar(&Settings::new(None, tab), width, &theme);
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn a_separator_only_list_unsets_instead_of_saving_an_empty_array() {
        for text in [",", ", ,", ""] {
            assert_eq!(typed(&SettingKind::List, text), Ok(Value::Null), "{text:?}");
        }
        assert_eq!(
            typed(&SettingKind::List, "a, b"),
            Ok(Value::Array(vec!["a".into(), "b".into()]))
        );
    }

    #[test]
    fn a_wide_strip_shows_every_tab_and_a_narrow_one_keeps_the_current_in_view() {
        let wide = strip(SESSION, 80);
        for tab in 0..=KEYS {
            assert!(wide.contains(tab_name(tab)), "{wide}");
        }
        assert!(!wide.contains('‹') && !wide.contains('›'), "{wide}");
        let narrow = strip(5, 30);
        assert!(narrow.contains(" Budget "), "{narrow}");
        assert!(
            narrow.starts_with("‹ ") && narrow.ends_with(" ›"),
            "{narrow}"
        );
        assert!(narrow.chars().count() <= 30, "{narrow}");
    }

    /// A provider list typed as "DeepInfra, Together" must save as the array OpenRouter takes,
    /// and reopen as the same text rather than as JSON.
    #[test]
    fn a_list_row_is_typed_as_comma_separated_names_and_saves_an_array() {
        use ratatui::crossterm::event::KeyModifiers;
        let press = |code| KeyEvent::new(code, KeyModifiers::NONE);
        let mut rows = yi_types::settings::rows();
        let at = rows
            .iter()
            .position(|row| row.id == "routing.order")
            .unwrap_or(0);
        rows[at].value = serde_json::json!(["Together"]);
        let tab = TABS
            .iter()
            .position(|tab| *tab == rows[at].tab)
            .unwrap_or(0)
            + 1;
        let mut settings = Settings::new(None, tab);
        settings.rows = Some(rows);
        settings.cursor = settings
            .tab_rows()
            .iter()
            .position(|row| row.id == "routing.order")
            .unwrap_or(0);
        assert!(matches!(
            key(&mut settings, press(KeyCode::Enter), &[]),
            Intent::None
        ));
        assert!(matches!(&settings.edit, Some(Edit::Type(text)) if text == "Together"));
        for ch in ", DeepInfra ,".chars() {
            key(&mut settings, press(KeyCode::Char(ch)), &[]);
        }
        let saved = key(&mut settings, press(KeyCode::Enter), &[]);
        assert!(matches!(
            saved,
            Intent::SaveDefault { ref id, ref value }
                if id == "routing.order" && *value == serde_json::json!(["Together", "DeepInfra"])
        ));
    }

    /// A list row the daemon redacted to its entry count (`skills.global`, `lanes.land`) must
    /// not open the comma editor on that count: Enter and `u` both point at config.json.
    #[test]
    fn a_redacted_list_row_points_at_config_json() -> Result<(), Box<dyn std::error::Error>> {
        use ratatui::crossterm::event::KeyModifiers;
        fn not_edited(id: &str, value: Value) -> Result<(), Box<dyn std::error::Error>> {
            let press = |code| KeyEvent::new(code, KeyModifiers::NONE);
            let mut rows = yi_types::settings::rows();
            let at = rows.iter().position(|row| row.id == id).ok_or("no row")?;
            rows[at].value = value;
            let tab = TABS
                .iter()
                .position(|tab| *tab == rows[at].tab)
                .ok_or("no tab")?
                + 1;
            let mut settings = Settings::new(None, tab);
            settings.rows = Some(rows);
            settings.cursor = settings
                .tab_rows()
                .iter()
                .position(|row| row.id == id)
                .ok_or("no row")?;
            let opened = key(&mut settings, press(KeyCode::Enter), &[]);
            let unset = key(&mut settings, press(KeyCode::Char('u')), &[]);
            assert!(matches!(opened, Intent::Note(_)), "{id} opens an editor");
            assert!(settings.edit.is_none(), "{id} opened an editor");
            assert!(matches!(unset, Intent::Note(_)), "{id} unsets");
            Ok(())
        }
        not_edited("skills.global", serde_json::json!(2))?;
        not_edited("lanes.land", Value::Null)
    }
}
