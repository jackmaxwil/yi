//! The rail and the full sidebar: rows by recency, the root filter, the edge mark.
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use yi_tui::colors::{Theme, name_tile, tile_style};

use crate::app::App;
use crate::model::{Mode, SessionStatus, SidebarMode, Zone, now_ms};
use crate::render::{NAME_WIDTH, RAIL_ROWS, status_style};

fn bucket(now: u64, then: u64) -> &'static str {
    if then == 0 {
        return "older";
    }
    match now.saturating_sub(then) / 1000 {
        0..3600 => "this hour",
        3600..86_400 => "today",
        86_400..604_800 => "this week",
        _ => "older",
    }
}

fn fade(line: &mut Line<'static>, level: usize, theme: &Theme) {
    let mut style = theme.dim_style();
    if level > 1 {
        style = style.add_modifier(Modifier::DIM);
    }
    for span in &mut line.spans {
        span.style = span.style.patch(style);
    }
}

pub fn sidebar_lines(app: &App, theme: &Theme, height: u16) -> Vec<(Option<usize>, Line<'static>)> {
    let mut rows = Vec::new();
    let rail = app.state.sidebar == SidebarMode::Rail;
    let multi_root = !rail && app.state.roots().len() > 1;
    let mut current_root: Option<&str> = None;
    let mut current_bucket: Option<&str> = None;
    let focused = app.state.focused_session();
    let now = now_ms();
    let mut slot = 0_usize;
    let visible = app.state.visible_rows();
    let shown = if rail {
        visible.iter().copied().take(RAIL_ROWS).collect()
    } else {
        visible
    };
    for index in shown {
        let Some(id) = app.state.order.get(index) else {
            continue;
        };
        let Some(row) = app.state.sessions.get(id) else {
            continue;
        };
        if multi_root && current_root != Some(row.root.as_str()) {
            current_root = Some(row.root.as_str());
            let label = row.root.rsplit('/').next().unwrap_or(&row.root);
            rows.push((
                None,
                Line::styled(
                    format!(" {label}"),
                    theme.accent_style().add_modifier(Modifier::BOLD),
                ),
            ));
        }
        if !rail && current_bucket != Some(bucket(now, row.recency())) {
            current_bucket = Some(bucket(now, row.recency()));
            rows.push((
                None,
                Line::styled(
                    format!("  {}", bucket(now, row.recency())),
                    theme.dim_style(),
                ),
            ));
        }
        let is_focused = focused.as_ref() == Some(id);
        let is_cursor = index == app.state.selected && app.state.zone == Zone::Sidebar;
        slot = slot.saturating_add(1);
        let label = row.label();
        let row_bg = if is_cursor {
            Some(theme.selection_bg())
        } else if is_focused && !rail {
            Some(theme.active_row_bg())
        } else {
            None
        };
        let on_row = |style: Style| row_bg.map_or(style, |bg| style.bg(bg));
        let number = if is_cursor {
            theme.muted_style()
        } else if is_focused {
            Style::default().fg(theme.text)
        } else {
            theme.dim_style()
        };
        let slot_text = if slot <= 9 {
            format!("{slot:<2}")
        } else {
            "  ".to_owned()
        };
        if rail {
            rows.push((
                Some(index),
                Line::from(vec![
                    Span::styled(slot_text, on_row(number)),
                    Span::styled(name_tile(row.seed()), tile_style(row.seed())),
                    Span::styled("   ".to_owned(), on_row(Style::default())),
                    Span::styled(
                        row.status.glyph().to_owned(),
                        on_row(status_style(theme, row.status)),
                    ),
                ]),
            ));
            rows.push((
                Some(index),
                Line::from(Span::styled(" ".repeat(8), on_row(Style::default()))),
            ));
            continue;
        }
        let mut spans = vec![
            Span::styled(slot_text, on_row(number)),
            Span::styled(name_tile(row.seed()), tile_style(row.seed())),
            Span::styled(" ".to_owned(), on_row(Style::default())),
            Span::styled(
                row.status.glyph().to_owned(),
                on_row(status_style(theme, row.status)),
            ),
        ];
        let name: String = label.chars().take(NAME_WIDTH).collect();
        let pad = NAME_WIDTH.saturating_sub(name.chars().count());
        let name_style = if is_focused {
            theme.accent_style().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        let tail = if row.attached { "·" } else { " " };
        let icon = spans.pop();
        spans.pop();
        spans.push(Span::styled(" ".to_owned(), on_row(Style::default())));
        spans.push(Span::styled(name, on_row(name_style)));
        spans.push(Span::styled(
            format!("{} ", " ".repeat(pad)),
            on_row(Style::default()),
        ));
        spans.extend(icon);
        spans.push(Span::styled(tail.to_owned(), on_row(theme.dim_style())));
        rows.push((Some(index), Line::from(spans)));
        rows.extend(child_rows(app, id, theme));
    }
    window_rows(rows, height, app.state.selected, theme)
}

fn child_rows(
    app: &App,
    id: &crate::model::SessionId,
    theme: &Theme,
) -> Vec<(Option<usize>, Line<'static>)> {
    let mut rows = Vec::new();
    for child in app.state.children.get(id).into_iter().flatten().take(3) {
        let name: String = child
            .name
            .chars()
            .filter(|c| !c.is_control())
            .take(NAME_WIDTH)
            .collect();
        let glyph = match child.status {
            yi_types::subagent::ChildStatus::Running => "◐",
            yi_types::subagent::ChildStatus::Completed => "○",
            yi_types::subagent::ChildStatus::Error => "✕",
        };
        rows.push((
            None,
            Line::styled(format!("   └ {name} {glyph}"), theme.dim_style()),
        ));
    }
    rows
}

/// The rows that fit, keeping the cursor on screen, faded two deep at a cut edge.
fn window_rows(
    rows: Vec<(Option<usize>, Line<'static>)>,
    height: u16,
    selected: usize,
    theme: &Theme,
) -> Vec<(Option<usize>, Line<'static>)> {
    let visible = usize::from(height);
    if visible == 0 || rows.len() <= visible {
        return rows;
    }
    let cursor = rows
        .iter()
        .position(|(index, _)| *index == Some(selected))
        .unwrap_or(0);
    let start = cursor.saturating_add(1).saturating_sub(visible);
    let end = start.saturating_add(visible).min(rows.len());
    let below = end < rows.len();
    let mut window: Vec<(Option<usize>, Line<'static>)> =
        rows.get(start..end).map(<[_]>::to_vec).unwrap_or_default();
    let last = window.len().saturating_sub(1);
    for (offset, (_, line)) in window.iter_mut().enumerate() {
        let from_bottom = last.saturating_sub(offset);
        let level = if start > 0 && offset < 2 {
            2_usize.saturating_sub(offset)
        } else if below && from_bottom < 2 {
            2_usize.saturating_sub(from_bottom)
        } else {
            0
        };
        if level > 0 {
            fade(line, level, theme);
        }
    }
    window
}

pub(crate) fn render_roots(app: &App, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let mut lines = vec![Line::styled(
        " workspaces",
        theme.accent_style().add_modifier(Modifier::BOLD),
    )];
    for root in app.state.roots() {
        let worst = app
            .state
            .sessions
            .values()
            .filter(|row| row.root == root)
            .map(|row| row.status)
            .min_by_key(|status| *status as u8)
            .unwrap_or(SessionStatus::Unknown);
        let count = app
            .state
            .sessions
            .values()
            .filter(|row| row.root == root)
            .count();
        let marker = if app.state.root_filter.as_deref() == Some(root.as_str()) {
            "▸"
        } else {
            " "
        };
        let label: String = root
            .rsplit('/')
            .next()
            .unwrap_or(&root)
            .chars()
            .take(usize::from(area.width).saturating_sub(6))
            .collect();
        lines.push(Line::from(vec![
            Span::styled(marker.to_owned(), theme.accent_style()),
            Span::styled(format!("{} ", worst.glyph()), status_style(theme, worst)),
            Span::styled(label, Style::default().fg(theme.text)),
            Span::styled(format!(" {count}"), theme.dim_style()),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

pub(crate) fn render_sidebar(app: &App, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let rows = sidebar_lines(app, theme, area.height);
    let mut lines: Vec<Line<'static>> = rows.iter().map(|(_, line)| line.clone()).collect();
    if lines.is_empty() && app.state.sidebar == SidebarMode::Full {
        let label = app
            .state
            .root
            .rsplit('/')
            .next()
            .unwrap_or(app.state.root.as_str());
        lines.push(Line::styled(
            format!(" {label}"),
            theme.accent_style().add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::styled(" no sessions yet", theme.dim_style()));
    }
    frame.render_widget(Paragraph::new(lines), area);
    if app.state.sidebar == SidebarMode::Rail {
        let armed = app.state.zone == Zone::Sidebar || matches!(app.state.mode, Mode::Prefix);
        let style = if armed {
            theme.accent_style()
        } else {
            theme.dim_style()
        };
        let focused = app
            .state
            .focused_session()
            .and_then(|id| app.state.order.iter().position(|row| *row == id));
        let x = area.x.saturating_add(area.width).saturating_sub(1);
        let buffer = frame.buffer_mut();
        for (offset, (index, _)) in rows.iter().enumerate() {
            let Some(y) = u16::try_from(offset)
                .ok()
                .and_then(|o| area.y.checked_add(o))
            else {
                break;
            };
            if y >= area.bottom() {
                break;
            }
            let in_front = index.is_some() && *index == focused;
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_symbol(if in_front { "┃" } else { "│" });
                cell.set_style(if in_front {
                    theme.accent_style()
                } else {
                    style
                });
            }
        }
        for y in area
            .y
            .saturating_add(u16::try_from(rows.len()).unwrap_or(0))..area.bottom()
        {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_symbol("│");
                cell.set_style(style);
            }
        }
    }
}
