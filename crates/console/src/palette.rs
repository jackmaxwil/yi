//! The command palette and the keys overlay: popups over the panes.
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use yi_tui::colors::{Theme, name_tile, tile_style_at};

use crate::app::App;
use crate::keys;
use crate::model::{Mode, SessionStatus};
use crate::render::{ViewState, label_of, status_style};

fn popup_in(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x.saturating_add(area.width.saturating_sub(width) / 2),
        y: area.y.saturating_add(1),
        width,
        height,
    }
}

const DISMISS: &str = " esc ✕ ";

pub(crate) fn dismiss_mark(popup: Rect) -> Rect {
    let width = u16::try_from(DISMISS.chars().count()).unwrap_or(7);
    Rect::new(
        popup.right().saturating_sub(width.saturating_add(1)),
        popup.y,
        width,
        1,
    )
}

pub(crate) fn render_navigator(
    app: &App,
    frame: &mut Frame<'_>,
    view: &ViewState,
    theme: &Theme,
) -> Option<Rect> {
    let Mode::Navigator { query, selected } = &app.state.mode else {
        return None;
    };
    let area = view.panes_area;
    let width = area.width.saturating_sub(8).clamp(44, 72).min(area.width);
    let height = area.height.saturating_sub(4).clamp(6, 22).min(area.height);
    let popup = popup_in(area, width, height);
    frame.render_widget(ratatui::widgets::Clear, popup);
    let block = ratatui::widgets::Block::bordered()
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(theme.muted_style())
        .title(" Command Palette ")
        .title_top(Line::from(DISMISS).right_aligned());
    let inner = usize::from(width.saturating_sub(4));
    let amber = Style::default().fg(theme.warning);
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" › ", amber),
            Span::styled(query.clone(), Style::default().fg(theme.text)),
            Span::styled("▌", amber),
        ]),
        Line::default(),
    ];
    let entries = app.palette_entries(query);
    let visible = usize::from(height.saturating_sub(5));
    let picked_index = (*selected).min(entries.len().saturating_sub(1));
    let first = picked_index.saturating_sub(visible.saturating_sub(1));
    let mut section: Option<&str> = None;
    for (index, entry) in entries.iter().enumerate().skip(first).take(visible) {
        let picked = index == picked_index;
        let marker = if picked { " ▸ " } else { "   " };
        let row_style = if picked {
            Style::default()
                .fg(theme.text)
                .bg(theme.selection_bg())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        let (heading, spans) = match entry {
            crate::app::PaletteEntry::Action { index } => {
                let Some(chord) = keys::CHORDS.get(*index) else {
                    continue;
                };
                let key = chord.key(app.cmd_hints);
                let pad =
                    inner.saturating_sub(chord.what.chars().count() + key.chars().count() + 4);
                (
                    "actions",
                    vec![
                        Span::styled(marker, row_style),
                        Span::styled(chord.what, row_style),
                        Span::styled(format!("{}{key}", " ".repeat(pad)), theme.muted_style()),
                    ],
                )
            }
            crate::app::PaletteEntry::Session(id) => {
                let row_status = app
                    .state
                    .sessions
                    .get(id)
                    .map_or(SessionStatus::Unknown, |row| row.status);
                let seed = app
                    .state
                    .sessions
                    .get(id)
                    .map_or_else(|| id.0.clone(), |row| row.seed().to_owned());
                let hue = app.accent_of(&id.0, &seed);
                (
                    "sessions",
                    vec![
                        Span::styled(marker, row_style),
                        Span::styled(name_tile(&seed), tile_style_at(hue)),
                        Span::styled(
                            format!(" {} ", row_status.glyph()),
                            status_style(theme, row_status),
                        ),
                        Span::styled(
                            label_of(&app.state.sessions, id, inner.saturating_sub(8)),
                            row_style,
                        ),
                    ],
                )
            }
        };
        if section != Some(heading) {
            section = Some(heading);
            lines.push(Line::styled(format!(" {heading}"), theme.dim_style()));
        }
        let mut line = Line::from(spans);
        if picked {
            let rest = usize::from(width.saturating_sub(2)).saturating_sub(line.width());
            line.push_span(Span::styled(" ".repeat(rest), row_style));
        }
        lines.push(line);
    }
    if entries.is_empty() {
        lines.push(Line::styled(
            "   nothing matches — `e path` opens a file, `md path`, `diff path`, `nb`",
            theme.dim_style(),
        ));
    }
    frame.render_widget(Paragraph::new(lines).block(block), popup);
    Some(popup)
}
