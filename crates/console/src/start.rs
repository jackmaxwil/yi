//! What an empty chat or notebook pane shows above its composer: the keys that start work and the
//! sessions to go back to. Every row is a control in the table the header buttons already use.

use std::cmp::Reverse;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use yi_tui::colors::Theme;

use crate::app::App;
use crate::hit::Hit;
use crate::keys::{self, Action};
use crate::layout::PaneId;
use crate::model::{PaneContent, SessionId, now_ms};

const WIDTH: u16 = 44;
const RECENT: usize = 5;
const CHAT: [(Action, &str); 6] = [
    (Action::Navigator, "command palette"),
    (Action::NewSession, "new session"),
    (Action::SplitRight, "split right"),
    (Action::OpenEditor, "files"),
    (Action::ToggleNotebook, "notebook"),
    (Action::Keys, "all keys"),
];
const NOTEBOOK: [(Action, &str); 3] = [
    (Action::ToggleNotebook, "close notebook"),
    (Action::NewSession, "new session"),
    (Action::Navigator, "command palette"),
];

type Row = (Line<'static>, Option<Hit>);

fn keys_rows(app: &App, list: &[(Action, &str)], theme: &Theme) -> Vec<Row> {
    let cap = Style::default().fg(theme.text).add_modifier(Modifier::BOLD);
    list.iter()
        .map(|(action, label)| {
            let chord = keys::chord_for(*action, app.cmd_hints);
            let line = Line::from(vec![
                Span::styled(format!("{chord:<4}"), cap),
                Span::styled(label.to_string(), theme.muted_style()),
            ]);
            (line, Some(Hit::Button(*action)))
        })
        .collect()
}

/// Named sessions other than `own`, newest first, each with the sidebar index a press opens.
fn recent_rows(app: &App, own: Option<&SessionId>, width: usize, theme: &Theme) -> Vec<Row> {
    let mut named: Vec<_> = (app.state.sessions.values())
        .filter(|row| row.name.is_some() && Some(&row.id) != own)
        .collect();
    named.sort_by_key(|row| Reverse(row.recency()));
    let now = now_ms();
    let room = width.saturating_sub(8);
    named
        .into_iter()
        .filter_map(|row| {
            let index = app.state.order.iter().position(|id| *id == row.id)?;
            let age = crate::sidebar::age(now.saturating_sub(row.recency()) / 1000);
            let label: String = row.label().chars().take(room).collect();
            let line = Line::from(vec![
                Span::styled(format!("{label:<room$} "), Style::default().fg(theme.text)),
                Span::styled(format!("{age:>4}"), theme.dim_style()),
            ]);
            Some((line, Some(Hit::Session(index))))
        })
        .take(RECENT)
        .collect()
}

fn blank() -> Row {
    (Line::default(), None)
}

fn paint(app: &mut App, buffer: &mut Buffer, area: Rect, rows: Vec<Row>) {
    let height = u16::try_from(rows.len())
        .unwrap_or(u16::MAX)
        .min(area.height);
    let width = WIDTH.min(area.width);
    let x = area.x.saturating_add(area.width.saturating_sub(width) / 2);
    let top = area
        .y
        .saturating_add(area.height.saturating_sub(height) / 2);
    for (offset, (line, hit)) in (0..height).zip(rows) {
        let rect = Rect::new(x, top.saturating_add(offset), width, 1);
        Paragraph::new(line).render(rect, buffer);
        if let (Some(hit), Some(hits)) = (hit, app.hits.as_mut()) {
            hits.controls.push((rect, hit));
        }
    }
}

fn title(lines: &[&'static str], theme: &Theme) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    rows.extend(
        lines
            .iter()
            .map(|text| (Line::styled(*text, theme.muted_style()), None)),
    );
    rows.push(blank());
    rows
}

/// A chat whose transcript is empty paints this in the `rows` above its composer.
pub(crate) fn chat(
    app: &mut App,
    buffer: &mut Buffer,
    inner: Rect,
    pane: PaneId,
    blank_rows: Option<u16>,
    theme: &Theme,
) {
    let Some(rows) = blank_rows else { return };
    let own = app
        .state
        .panes
        .get(&pane)
        .and_then(|pane| pane.session())
        .cloned();
    let mut all = title(&["ask below, or start with"], theme);
    all.extend(keys_rows(app, &CHAT, theme));
    let recent = recent_rows(app, own.as_ref(), usize::from(WIDTH), theme);
    if !recent.is_empty() {
        all.push(blank());
        all.push((Line::styled("recent", theme.dim_style()), None));
        all.extend(recent);
    }
    paint(
        app,
        buffer,
        Rect {
            height: rows,
            ..inner
        },
        all,
    );
}

/// A notebook with no cells yet.
pub(crate) fn notebook(
    app: &mut App,
    buffer: &mut Buffer,
    pane: PaneId,
    inner: Rect,
    theme: &Theme,
) {
    let empty = match app.state.panes.get(&pane).map(|pane| &pane.content) {
        Some(PaneContent::Notebook { session, .. }) => session
            .as_ref()
            .is_none_or(|id| crate::model::notebook_cells(&app.state.notebooks, id).is_empty()),
        _ => false,
    };
    if !empty {
        return;
    }
    let mut all = title(
        &["no kernel cells yet", "they appear as the agent computes"],
        theme,
    );
    all.extend(keys_rows(app, &NOTEBOOK, theme));
    paint(app, buffer, inner, all);
}
