//! The settings pane: the focused session's live options, every config default the daemon
//! serves, and the key table, a tab each.
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use yi_tui::colors::Theme;
use yi_tui::theme_file::{BUILT_IN, ground_id, is_theme_row};
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
    /// `~/.yi/themes`, listed beside the shipped themes each time a theme row opens.
    pub themes: Option<std::path::PathBuf>,
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
            themes: None,
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

/// A theme painted from an open list, and the palette any close but a save puts back.
pub struct ThemePreview {
    pub restore: Theme,
    pub pane: crate::layout::PaneId,
    pub id: String,
    pub shown: String,
    pub warned: bool,
    /// Enter sent the save; the palette now waits on the daemon's answer, not on the list.
    pub saving: bool,
}

/// What a key asks the app to send; the pane itself only moves its cursor and edit.
pub enum Intent {
    None,
    SaveDefault { id: String, value: Value },
    SetSession { config_id: String, value: String },
    PreviewTheme { id: String, name: String },
    RestoreTheme,
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

pub(crate) fn count(settings: &Settings, options: &[AcpConfigOption]) -> usize {
    match settings.tab {
        SESSION => options.len(),
        KEYS => crate::keys::CHORDS.len(),
        _ => settings.tab_rows().len(),
    }
}

fn waiting(settings: &Settings) -> bool {
    settings.tab != SESSION && settings.tab != KEYS && settings.rows.is_none()
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

/// The control a press works: a key opens a choice's picker, a click steps to the next value and
/// saves it, so no setting on the page needs a key.
#[derive(Clone, Copy, PartialEq, Eq)]
enum How {
    Key,
    Click,
}

fn theme_row(settings: &Settings) -> Option<String> {
    let rows = settings.tab_rows();
    let row = rows.get(settings.cursor)?;
    is_theme_row(&row.id).then(|| row.id.clone())
}

pub(crate) fn theme_list_open(settings: &Settings) -> Option<String> {
    theme_row(settings).filter(|_| matches!(settings.edit, Some(Edit::Pick { .. })))
}

/// The value a press moves a choice to: the one after the current, wrapping.
fn stepped(picks: &[String], current: &str) -> Option<String> {
    if picks.len() < 2 {
        return None;
    }
    let at = picks
        .iter()
        .position(|pick| pick == current)
        .map_or(0, |at| at.saturating_add(1) % picks.len());
    picks.get(at).cloned()
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
            let theme = theme_row(settings);
            if let Some(cursor) = next {
                let name = picks.get(cursor).cloned();
                settings.edit = Some(Edit::Pick {
                    options: picks,
                    cursor,
                });
                return match (theme, name) {
                    (Some(id), Some(name)) => Intent::PreviewTheme { id, name },
                    _ => Intent::None,
                };
            }
            return match (theme, chosen) {
                (Some(_), None) => Intent::RestoreTheme,
                (Some(_), Some(choice)) if choice == BUILT_IN => {
                    commit(settings, options, Value::Null)
                }
                (_, Some(choice)) => commit(settings, options, Value::String(choice)),
                (None, None) => Intent::None,
            };
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
        KeyCode::Enter => begin(settings, options, How::Key),
        KeyCode::Char(' ') => begin(settings, options, How::Click),
        KeyCode::Char('u') if settings.tab != SESSION && settings.tab != KEYS => {
            let Some(control) = settings
                .tab_rows()
                .get(settings.cursor)
                .map(|row| control(row))
            else {
                return Intent::None;
            };
            if control == Control::Note {
                let Some(id) = settings
                    .tab_rows()
                    .get(settings.cursor)
                    .map(|row| row.id.clone())
                else {
                    return Intent::None;
                };
                Intent::Note(format!("{id} is edited in ~/.yi/config.json"))
            } else {
                commit(settings, options, Value::Null)
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

/// A press on row `row`: the pane focuses it and works its control, so no setting needs a key.
pub(crate) fn click(settings: &mut Settings, row: usize, options: &[AcpConfigOption]) -> Intent {
    settings.cursor = row;
    (settings.edit, settings.status) = (None, None);
    begin(settings, options, How::Click)
}

fn begin(settings: &mut Settings, options: &[AcpConfigOption], how: How) -> Intent {
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
        if how == How::Click {
            return stepped(&picks, &option.current_value).map_or(Intent::None, |value| {
                Intent::SetSession {
                    config_id: option.config_id.clone(),
                    value,
                }
            });
        }
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
    match control(&row) {
        Control::Step => {
            let on = if row.value.is_null() {
                &row.default
            } else {
                &row.value
            };
            commit(settings, options, Value::Bool(on != &Value::Bool(true)))
        }
        Control::Pick if is_theme_row(&row.id) => {
            let current = row.value.as_str().unwrap_or(BUILT_IN);
            let picks = yi_tui::theme_file::list(settings.themes.as_deref());
            let cursor = picks.iter().position(|pick| pick == current).unwrap_or(0);
            settings.edit = Some(Edit::Pick {
                options: picks,
                cursor,
            });
            Intent::None
        }
        Control::Pick => {
            let SettingKind::Choice { options: picks } = &row.kind else {
                return Intent::None;
            };
            let current = row.value.as_str().unwrap_or_default();
            if how == How::Click {
                return stepped(picks, current).map_or(Intent::None, |value| {
                    commit(settings, options, Value::String(value))
                });
            }
            let cursor = picks.iter().position(|pick| pick == current).unwrap_or(0);
            settings.edit = Some(Edit::Pick {
                options: picks.clone(),
                cursor,
            });
            Intent::None
        }
        Control::Edit => {
            let text = if row.value.is_null() {
                String::new()
            } else {
                shown_value(&row.value)
            };
            settings.edit = Some(Edit::Type(text));
            Intent::None
        }
        Control::Note => Intent::Note(format!("{} is edited in ~/.yi/config.json", row.id)),
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
        (Value::Null, _) if is_theme_row(&row.id) => BUILT_IN.to_owned(),
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

#[derive(Clone, Copy, Eq, PartialEq)]
enum Control {
    Step,
    Pick,
    Edit,
    Note,
}

fn control(row: &SettingRow) -> Control {
    if is_theme_row(&row.id) {
        return Control::Pick;
    }
    match &row.kind {
        SettingKind::Toggle => Control::Step,
        SettingKind::Choice { .. } => Control::Pick,
        SettingKind::Number { .. } | SettingKind::Text | SettingKind::Model => Control::Edit,
        SettingKind::List
            if pane_edits_list(&row.id)
                && (row.value.is_null()
                    || row
                        .value
                        .as_array()
                        .is_some_and(|items| items.iter().all(Value::is_string))) =>
        {
            Control::Edit
        }
        SettingKind::List | SettingKind::Json => Control::Note,
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

/// The strip's pieces and the tab each selects, so the drawn strip and its click targets agree.
fn strip(settings: &Settings, width: usize) -> Vec<(String, Option<usize>)> {
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
    let mut pieces = vec![((if first > 0 { "‹ " } else { "  " }).to_owned(), None)];
    for index in first..=last {
        if index > first {
            pieces.push((" ".to_owned(), None));
        }
        let name = if index == settings.tab {
            format!(" {} ", tab_name(index))
        } else {
            tab_name(index).to_owned()
        };
        pieces.push((name, Some(index)));
    }
    pieces.push(((if last < KEYS { " ›" } else { "" }).to_owned(), None));
    pieces
}

fn tab_bar(settings: &Settings, width: usize, theme: &Theme) -> Line<'static> {
    let chip = Style::default()
        .fg(theme.text)
        .bg(theme.selection_bg())
        .add_modifier(Modifier::BOLD);
    let spans = strip(settings, width).into_iter().map(|(text, tab)| {
        let current = tab == Some(settings.tab);
        Span::styled(text, if current { chip } else { theme.dim_style() })
    });
    Line::from(spans.collect::<Vec<_>>())
}

/// A click target of the pane: a tab, or a row, so the drawing and the hit table are one table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Tab(usize),
    Row(usize),
}

/// (rect, pane, target) of every click target of every settings pane on screen.
pub(crate) fn hits(
    app: &crate::app::App,
    panes: &[crate::render::PaneView],
) -> Vec<(Rect, crate::layout::PaneId, Target)> {
    let mut hits = Vec::new();
    for view in panes {
        let Some(crate::model::PaneContent::Settings(settings)) =
            app.state.panes.get(&view.id).map(|pane| &pane.content)
        else {
            continue;
        };
        let options = settings
            .session
            .as_ref()
            .and_then(|id| app.state.session_options.get(id));
        let picks = options.map_or(&[][..], Vec::as_slice);
        for (rect, target) in targets(settings, picks, view.rect.inner(view.margin)) {
            hits.push((rect, view.id, target));
        }
    }
    hits
}

/// Where every target sits inside the pane: the strip's tabs, then every row the frame draws.
pub(crate) fn targets(
    settings: &Settings,
    options: &[AcpConfigOption],
    inner: Rect,
) -> Vec<(Rect, Target)> {
    let mut out = Vec::new();
    let mut x = 0_u16;
    for (text, tab) in strip(settings, usize::from(inner.width)) {
        let cells = u16::try_from(Line::raw(text).width()).unwrap_or(u16::MAX);
        out.extend(tab.map(|tab| {
            let rect = Rect::new(inner.x.saturating_add(x), inner.y, cells, 1).intersection(inner);
            (rect, Target::Tab(tab))
        }));
        x = x.saturating_add(cells);
    }
    let total = count(settings, options);
    let (first, room) = window(settings, usize::from(inner.height));
    let top = inner.y.saturating_add(2);
    let listed = list_height(settings);
    for row in first..total.min(first.saturating_add(room)) {
        let below = if row > settings.cursor { listed } else { 0 };
        let offset = u16::try_from(row.saturating_sub(first).saturating_add(below)).unwrap_or(0);
        out.push((
            Rect::new(inner.x, top.saturating_add(offset), inner.width, 1),
            Target::Row(row),
        ));
    }
    out
}

/// (label, value, when it applies, help, default state, control) of every row on the current tab.
type Shown = (String, String, String, String, bool, Control);

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
                (
                    option.name.clone(),
                    named,
                    "now".to_owned(),
                    help,
                    false,
                    Control::Pick,
                )
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
                    false,
                    Control::Note,
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
                    row.value.is_null() || row.value == row.default,
                    control(row),
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
    let mut lines = vec![
        tab_bar(settings, width, theme),
        Line::styled("─".repeat(width), theme.dim_style()),
    ];
    let rows = shown_rows(settings, options, cmd);
    let (first, room) = window(settings, visible);
    if waiting(settings) {
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
    for (index, (name, shown, applies, _, at_default, _)) in
        rows.iter().enumerate().skip(first).take(room)
    {
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
        let bar = if chosen {
            Style::default().bg(theme.selection_bg())
        } else {
            Style::default()
        };
        let label_style = if chosen {
            bar.fg(theme.text).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        let value_style = if editing.is_some() {
            bar.fg(theme.accent).add_modifier(Modifier::BOLD)
        } else if *at_default {
            bar.patch(theme.dim_style())
        } else {
            bar.fg(theme.teal)
        };
        let mut spans = vec![
            Span::styled(
                format!(" {marker}{}", clip(name, label.saturating_sub(3))),
                label_style,
            ),
            Span::styled(clip(&shown, value), value_style),
            Span::styled(clip(applies, tail), bar.patch(theme.dim_style())),
        ];
        let used: usize = spans.iter().map(|span| span.content.chars().count()).sum();
        spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), bar));
        lines.push(Line::from(spans));
        if let (true, Some((start, shown, total))) = (chosen, list_span(settings)) {
            lines.extend(listed(settings, start, shown, theme));
            if shown < total {
                let more = format!(
                    "        … {} more · ↑↓ to reach",
                    total.saturating_sub(shown)
                );
                lines.push(Line::styled(more, theme.dim_style()));
            }
        }
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
        .map_or("", |(_, _, _, help, _, _)| help.as_str());
    lines.extend(
        yi_tui::wrap::wrap_line(&Line::raw(help), width.saturating_sub(1), " ")
            .into_iter()
            .take(3),
    );
    if let Some(status) = &settings.status {
        lines.push(Line::styled(
            format!(" {status}"),
            Style::default().fg(theme.warning),
        ));
    }
    let listing = theme_list_open(settings).map(|id| id == ground_id(theme.dark));
    let control = rows
        .get(settings.cursor)
        .map_or(Control::Note, |(_, _, _, _, _, control)| *control);
    let hint = match (&settings.edit, settings.tab) {
        (Some(Edit::Type(_)), _) => " type · enter save · empty unsets · esc cancel",
        (Some(Edit::Pick { .. }), _) if listing == Some(true) => {
            " ↑↓ preview · enter save · esc restore"
        }
        (Some(Edit::Pick { .. }), _) if listing == Some(false) && theme.dark => {
            " ↑↓ choose · enter saves it for a light terminal · esc cancel"
        }
        (Some(Edit::Pick { .. }), _) if listing == Some(false) => {
            " ↑↓ choose · enter saves it for a dark terminal · esc cancel"
        }
        (Some(Edit::Pick { .. }), _) => " ↑↓ choose · enter save · esc cancel",
        (None, SESSION) => " ↑↓ move · ←→ tab · space next · enter pick · d make default",
        (None, KEYS) => " ↑↓ move · ←→ tab",
        (None, _) => match control {
            Control::Step => " ↑↓ move · ←→ tab · space toggle · u unset",
            Control::Pick => " ↑↓ move · ←→ tab · space next · enter pick · u unset",
            Control::Edit => " ↑↓ move · ←→ tab · enter edit · u unset",
            Control::Note => " ↑↓ move · ←→ tab · enter config.json",
        },
    };
    lines.push(Line::styled(hint, theme.dim_style()));
    lines
}

/// The open theme list under its row: the first name shown, how many, and how many exist.
fn list_span(settings: &Settings) -> Option<(usize, usize, usize)> {
    theme_list_open(settings)?;
    let Some(Edit::Pick { options, cursor }) = &settings.edit else {
        return None;
    };
    let (start, shown) = yi_tui::popup::window(*cursor, options.len());
    Some((start, shown, options.len()))
}

fn list_height(settings: &Settings) -> usize {
    list_span(settings).map_or(0, |(_, shown, total)| shown + usize::from(shown < total))
}

fn listed(settings: &Settings, start: usize, shown: usize, theme: &Theme) -> Vec<Line<'static>> {
    let Some(Edit::Pick { options, cursor }) = &settings.edit else {
        return Vec::new();
    };
    let row = |(index, name): (usize, &String)| {
        let (mark, style) = if index == *cursor {
            ("▸", theme.accent_style().add_modifier(Modifier::BOLD))
        } else {
            (" ", theme.muted_style())
        };
        Line::styled(format!("      {mark} {name}"), style)
    };
    options
        .iter()
        .enumerate()
        .skip(start)
        .take(shown)
        .map(row)
        .collect()
}

/// The row window `view` draws and a press resolves against: the first row and how many fit.
fn window(settings: &Settings, visible: usize) -> (usize, usize) {
    let room = visible
        .saturating_sub(8)
        .saturating_sub(list_height(settings))
        .max(1);
    (crate::panes::first_row(settings.cursor, room), room)
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

    /// A pane narrower than the current tab's name keeps that tab whole in the drawing; its
    /// target must stop at the pane's own edge rather than reach the split and the neighbour.
    #[test]
    fn a_tab_target_never_reaches_past_its_pane() {
        let inner = Rect::new(5, 0, 6, 10);
        let hits = targets(&Settings::new(None, 2), &[], inner);
        assert!(!hits.is_empty(), "the narrow pane lost its tab targets");
        for (rect, _) in hits {
            assert!(
                rect.x.saturating_add(rect.width) <= inner.x.saturating_add(inner.width),
                "{rect:?} reaches past the pane {inner:?}"
            );
        }
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
            let hint = view(
                &settings,
                &[],
                false,
                80,
                40,
                &Theme::new(ColorTier::TrueColor, true),
            )
            .last()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .ok_or("no hint")?;
            assert_eq!(
                hint, " ↑↓ move · ←→ tab · enter config.json",
                "{id} advertises an editor it cannot open"
            );
            let opened = key(&mut settings, press(KeyCode::Enter), &[]);
            let unset = key(&mut settings, press(KeyCode::Char('u')), &[]);
            assert!(matches!(opened, Intent::Note(_)), "{id} opens an editor");
            assert!(settings.edit.is_none(), "{id} opened an editor");
            assert!(matches!(unset, Intent::Note(_)), "{id} unsets");
            Ok(())
        }
        not_edited("skills.global", serde_json::json!(2))?;
        not_edited("lanes.land", Value::Null)?;
        not_edited("keys", Value::Null)
    }
}

#[cfg(test)]
mod theme_rows {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;
    use yi_tui::colors::ColorTier;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn press(settings: &mut Settings, code: KeyCode) -> Intent {
        key(settings, KeyEvent::new(code, KeyModifiers::NONE), &[])
    }

    fn on_dark_theme() -> Result<Settings, Box<dyn std::error::Error>> {
        let rows = yi_types::settings::rows();
        let row = rows
            .iter()
            .find(|row| row.id == "theme.dark")
            .ok_or("no theme.dark")?;
        let tab = TABS
            .iter()
            .position(|tab| *tab == row.tab)
            .ok_or("no tab")?
            + 1;
        let mut settings = Settings::new(None, tab);
        settings.rows = Some(rows);
        let at = settings
            .tab_rows()
            .iter()
            .position(|row| row.id == "theme.dark");
        settings.cursor = at.ok_or("no row")?;
        Ok(settings)
    }

    fn text(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn a_press_below_an_open_theme_list_lands_on_the_row_drawn_there() -> TestResult {
        let mut settings = on_dark_theme()?;
        assert!(matches!(press(&mut settings, KeyCode::Enter), Intent::None));
        let theme = Theme::new(ColorTier::TrueColor, true);
        let inner = Rect::new(0, 0, 80, 40);
        let lines = view(&settings, &[], false, 80, 40, &theme);
        let labels: Vec<String> = settings
            .tab_rows()
            .iter()
            .map(|row| row.label.clone())
            .collect();
        let mut below = 0;
        for (rect, target) in targets(&settings, &[], inner) {
            let Target::Row(row) = target else { continue };
            let drawn = lines.get(usize::from(rect.y)).map(text).unwrap_or_default();
            let label = labels.get(row).ok_or("a target past the rows")?;
            assert!(
                drawn.contains(label.as_str()),
                "row {row} ({label}) is drawn as {drawn:?}"
            );
            below += usize::from(row > settings.cursor);
        }
        assert!(below > 0, "the tab has rows under the theme row");
        let more = lines.iter().map(text).find(|line| line.contains("more ·"));
        assert_eq!(more.as_deref(), Some("        … 97 more · ↑↓ to reach"));
        Ok(())
    }

    #[test]
    fn space_changes_the_chosen_setting_like_enter() -> TestResult {
        let mut settings = on_dark_theme()?;
        press(&mut settings, KeyCode::Char(' '));
        assert!(settings.edit.is_some(), "space did not open the row");
        Ok(())
    }

    #[test]
    fn the_chosen_settings_row_is_a_full_width_bar_with_dim_defaults() -> TestResult {
        let mut settings = on_dark_theme()?;
        let theme = Theme::new(ColorTier::TrueColor, true);
        let bar = theme.selection_bg();
        let barred = |line: &&Line| {
            !line.spans.is_empty() && line.spans.iter().all(|span| span.style.bg == Some(bar))
        };
        let lines = view(&settings, &[], false, 80, 40, &theme);
        let chosen = lines.iter().find(barred).ok_or("no highlighted row")?;
        assert_eq!(text(chosen).chars().count(), 80, "the bar stops short");
        assert_eq!(
            lines.iter().filter(barred).count(),
            1,
            "more than one row is highlighted"
        );
        let value = chosen.spans.get(1).ok_or("no value span")?;
        assert_eq!(
            value.style.fg,
            theme.dim_style().fg,
            "the default is not dim"
        );
        let row = settings
            .rows
            .as_mut()
            .and_then(|rows| rows.iter_mut().find(|row| row.id == "theme.dark"))
            .ok_or("no theme.dark")?;
        row.value = Value::String("alabaster".to_owned());
        let lines = view(&settings, &[], false, 80, 40, &theme);
        let chosen = lines.iter().find(barred).ok_or("no highlighted row")?;
        let value = chosen.spans.get(1).ok_or("no value span")?;
        assert_eq!(
            value.style.fg,
            Some(theme.teal),
            "the changed value is not teal"
        );
        Ok(())
    }

    #[test]
    fn an_open_theme_list_keeps_the_footer_and_hint_inside_a_short_pane() -> TestResult {
        let mut settings = on_dark_theme()?;
        press(&mut settings, KeyCode::Enter);
        let theme = Theme::new(ColorTier::TrueColor, true);
        let lines = view(&settings, &[], false, 80, 20, &theme);
        let bar = theme.selection_bg();
        let chosen = lines
            .iter()
            .find(|line| {
                !line.spans.is_empty() && line.spans.iter().all(|span| span.style.bg == Some(bar))
            })
            .ok_or("no highlighted row")?;
        let value = chosen.spans.get(1).ok_or("no value span")?;
        assert_eq!(
            value.style.fg,
            Some(theme.accent),
            "the editing value is not accented"
        );
        let texts: Vec<String> = lines.iter().map(text).collect();
        assert!(
            lines.len() <= 20,
            "{} lines in a 20-row pane: {texts:#?}",
            lines.len()
        );
        let last = texts.last().map(String::as_str);
        assert_eq!(last, Some(" ↑↓ preview · enter save · esc restore"));
        Ok(())
    }

    #[test]
    fn the_other_grounds_list_says_it_only_saves() -> TestResult {
        let mut settings = on_dark_theme()?;
        settings.cursor = settings.cursor.saturating_add(1);
        press(&mut settings, KeyCode::Enter);
        let theme = Theme::new(ColorTier::TrueColor, true);
        let lines = view(&settings, &[], false, 80, 40, &theme);
        let hint = lines.last().map(text);
        let saves = " ↑↓ choose · enter saves it for a light terminal · esc cancel";
        assert_eq!(hint.as_deref(), Some(saves));
        Ok(())
    }

    #[test]
    fn a_theme_row_previews_on_move_and_saves_or_restores() -> TestResult {
        let mut settings = on_dark_theme()?;
        press(&mut settings, KeyCode::Enter);
        let preview = press(&mut settings, KeyCode::Down);
        assert!(
            matches!(&preview, Intent::PreviewTheme { id, name } if id == "theme.dark" && name == "alabaster")
        );
        let saved = press(&mut settings, KeyCode::Enter);
        assert!(
            matches!(&saved, Intent::SaveDefault { id, value } if id == "theme.dark" && value == "alabaster")
        );
        press(&mut settings, KeyCode::Enter);
        assert!(matches!(
            press(&mut settings, KeyCode::Esc),
            Intent::RestoreTheme
        ));
        press(&mut settings, KeyCode::Enter);
        let unset = press(&mut settings, KeyCode::Enter);
        assert!(matches!(&unset, Intent::SaveDefault { value, .. } if value.is_null()));
        Ok(())
    }
}
