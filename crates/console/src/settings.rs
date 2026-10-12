//! The settings pane: the focused session's live options, every config default the daemon
//! serves, and the key table, a tab each.
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use yi_tui::colors::Theme;
use yi_types::acp::AcpConfigOption;
use yi_types::settings::{Applies, SettingKind, SettingRow, TABS};

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
            commit(settings, options, Value::Null)
        }
        KeyCode::Char('d') if settings.tab == SESSION => {
            let option = options.get(settings.cursor);
            match option.and_then(|option| Some((default_of(&option.config_id)?, option))) {
                Some((id, option)) => Intent::SaveDefault {
                    id: id.to_owned(),
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
        SettingKind::Toggle => commit(
            settings,
            options,
            Value::Bool(row.value != Value::Bool(true)),
        ),
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

fn shown(value: &Value) -> String {
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
    let names: Vec<&str> = (0..=KEYS).map(tab_name).collect();
    let full: usize = names
        .iter()
        .map(|name| name.chars().count().saturating_add(2))
        .sum();
    if full > width {
        return Line::from(Span::styled(
            format!(" ◂ {} ▸", tab_name(settings.tab)),
            theme.accent_style().add_modifier(Modifier::BOLD),
        ));
    }
    let spans: Vec<Span<'static>> = names
        .iter()
        .enumerate()
        .map(|(tab, name)| {
            let style = if tab == settings.tab {
                theme
                    .accent_style()
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else {
                theme.dim_style()
            };
            Span::styled(format!(" {name} "), style)
        })
        .collect();
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
                    shown(&row.value),
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
