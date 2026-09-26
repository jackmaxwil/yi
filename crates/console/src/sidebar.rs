//! The rail and the full sidebar: rows by recency, the root filter, the edge mark.
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use yi_tui::colors::{Theme, accent_rgb, name_tile, tile_style_at};

use crate::app::App;
use crate::avatar::Placement;
use crate::model::{Mode, SessionStatus, SidebarMode, Zone, now_ms};
use crate::render::{NAME_WIDTH, status_style};

pub struct SidebarRow {
    pub index: Option<usize>,
    pub line: Line<'static>,
    pub avatar: Option<Placement>,
}

impl SidebarRow {
    fn plain(index: Option<usize>, line: Line<'static>) -> Self {
        Self {
            index,
            line,
            avatar: None,
        }
    }
}

fn avatar_at(col: u16, cols: u16, rows: u16, key: &str, hue: usize) -> Option<Placement> {
    Some(Placement {
        col,
        row: 0,
        cols,
        rows,
        key: key.to_owned(),
        seed: key.to_owned(),
        accent: accent_rgb(hue),
    })
}

fn tile_span(app: &App, text: String, hue: usize) -> Span<'static> {
    if app.kitty {
        Span::raw(" ".repeat(text.chars().count()))
    } else {
        Span::styled(text, tile_style_at(hue))
    }
}

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

pub const RAIL_CAP: usize = 9;

const NAME_FLOOR: usize = 10;

/// Incident: a fixed twenty-column name padded short names out to a gap wider than they are.
fn name_cols(app: &App) -> usize {
    let roots = app.state.roots().into_iter().map(|root| {
        let label = root.rsplit('/').next().unwrap_or(&root);
        label.chars().count().saturating_sub(3)
    });
    app.state
        .visible_rows()
        .into_iter()
        .filter_map(|index| app.state.order.get(index))
        .filter_map(|id| {
            let row = app.state.sessions.get(id)?;
            let children = app.state.children.get(id).into_iter().flatten().take(3);
            children
                .map(|child| child.name.chars().count().saturating_add(3))
                .chain(std::iter::once(row.label().chars().count()))
                .max()
        })
        .chain(roots)
        .max()
        .unwrap_or(0)
        .clamp(NAME_FLOOR, NAME_WIDTH)
        .max(app.state.sidebar_cols)
}

pub fn width(app: &mut App) -> u16 {
    match app.state.sidebar {
        SidebarMode::Rail => 9,
        SidebarMode::Full => {
            // Incident: narrowing as a turn's children cleared re-wrapped every pane mid-turn.
            app.state.sidebar_cols = name_cols(app);
            u16::try_from(app.state.sidebar_cols.saturating_add(9)).unwrap_or(u16::MAX)
        }
    }
}

pub fn sidebar_lines(app: &App, theme: &Theme, height: u16) -> Vec<SidebarRow> {
    let mut rows = Vec::new();
    let rail = app.state.sidebar == SidebarMode::Rail;
    let mut beyond = 0_usize;
    let multi_root = !rail && app.state.roots().len() > 1;
    let mut current_root: Option<&str> = None;
    let mut current_bucket: Option<&str> = None;
    let focused = app.state.focused_session();
    let now = now_ms();
    let mut slot = 0_usize;
    let cols = name_cols(app);
    for index in app.state.visible_rows() {
        let Some(id) = app.state.order.get(index) else {
            continue;
        };
        let Some(row) = app.state.sessions.get(id) else {
            continue;
        };
        if multi_root && current_root != Some(row.root.as_str()) {
            current_root = Some(row.root.as_str());
            let label = row.root.rsplit('/').next().unwrap_or(&row.root);
            rows.push(SidebarRow::plain(
                None,
                Line::styled(
                    format!(" {label}"),
                    theme.accent_style().add_modifier(Modifier::BOLD),
                ),
            ));
        }
        if !rail && current_bucket != Some(bucket(now, row.recency())) {
            current_bucket = Some(bucket(now, row.recency()));
            rows.push(SidebarRow::plain(
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
        if rail && slot > RAIL_CAP {
            beyond = beyond.saturating_add(1);
            continue;
        }
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
        let hue = app.accent_of(&id.0, row.seed());
        if rail {
            rows.push(SidebarRow {
                index: Some(index),
                line: Line::from(vec![
                    Span::styled(slot_text, on_row(number)),
                    tile_span(app, name_tile(row.seed()), hue),
                    Span::styled("   ".to_owned(), on_row(Style::default())),
                    Span::styled(
                        row.status.glyph().to_owned(),
                        on_row(status_style(theme, row.status)),
                    ),
                ]),
                avatar: avatar_at(2, 4, 2, &id.0, hue),
            });
            rows.push(SidebarRow::plain(
                Some(index),
                Line::from(Span::styled(" ".repeat(8), on_row(Style::default()))),
            ));
            rows.extend(child_rows(app, id, theme, true, cols));
            continue;
        }
        let mut spans = vec![
            Span::styled(slot_text, on_row(number)),
            tile_span(app, name_tile(row.seed()), hue),
            Span::styled(" ".to_owned(), on_row(Style::default())),
            Span::styled(
                row.status.glyph().to_owned(),
                on_row(status_style(theme, row.status)),
            ),
        ];
        let name: String = label.chars().take(cols).collect();
        let pad = cols.saturating_sub(name.chars().count());
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
        rows.push(SidebarRow {
            index: Some(index),
            line: Line::from(spans),
            avatar: avatar_at(2, 2, 1, &id.0, hue),
        });
        rows.extend(child_rows(app, id, theme, false, cols));
    }
    if beyond > 0 {
        rows.push(SidebarRow::plain(
            None,
            Line::styled(format!("  +{beyond}"), theme.dim_style()),
        ));
    }
    window_rows(rows, height, app.state.selected, theme)
}

fn child_rows(
    app: &App,
    id: &crate::model::SessionId,
    theme: &Theme,
    rail: bool,
    cols: usize,
) -> Vec<SidebarRow> {
    let mut rows = Vec::new();
    for child in app.state.children.get(id).into_iter().flatten().take(3) {
        let name: String = child
            .name
            .chars()
            .filter(|c| !c.is_control())
            .take(cols.saturating_sub(3))
            .collect();
        use yi_types::subagent::{ChildFlag, ChildStatus};
        let glyph = match (child.status, &child.flag) {
            (ChildStatus::Running, Some(ChildFlag::NeedsYou { .. })) => "?",
            (ChildStatus::Running, Some(ChildFlag::Stuck { .. })) => "!",
            (ChildStatus::Running, _) => "◐",
            (ChildStatus::Completed, _) => "○",
            (ChildStatus::Error, _) => "✕",
        };
        let hue = app.accent_of(child.id.as_str(), &child.name);
        let tile = tile_span(app, name_tile(&child.name), hue);
        let (spans, col) = if rail {
            (
                vec![
                    Span::styled("   ".to_owned(), theme.dim_style()),
                    tile,
                    Span::styled(format!(" {glyph}"), theme.dim_style()),
                ],
                3,
            )
        } else {
            (
                vec![
                    Span::styled("   └ ".to_owned(), theme.dim_style()),
                    tile,
                    Span::styled(format!(" {name} {glyph}"), theme.dim_style()),
                ],
                5,
            )
        };
        rows.push(SidebarRow {
            index: None,
            line: Line::from(spans),
            avatar: avatar_at(col, 2, 1, child.id.as_str(), hue),
        });
    }
    rows
}

/// The rows that fit, keeping the cursor on screen, faded two deep at a cut edge.
fn window_rows(
    rows: Vec<SidebarRow>,
    height: u16,
    selected: usize,
    theme: &Theme,
) -> Vec<SidebarRow> {
    let visible = usize::from(height);
    if visible == 0 || rows.len() <= visible {
        return rows;
    }
    let cursor = rows
        .iter()
        .position(|row| row.index == Some(selected))
        .unwrap_or(0);
    let start = cursor.saturating_add(1).saturating_sub(visible);
    let end = start.saturating_add(visible).min(rows.len());
    let below = end < rows.len();
    let mut rows = rows;
    let mut window: Vec<SidebarRow> = rows.drain(start..end).collect();
    let last = window.len().saturating_sub(1);
    for (offset, row) in window.iter_mut().enumerate() {
        let from_bottom = last.saturating_sub(offset);
        let level = if start > 0 && offset < 2 {
            2_usize.saturating_sub(offset)
        } else if below && from_bottom < 2 {
            2_usize.saturating_sub(from_bottom)
        } else {
            0
        };
        if level > 0 {
            fade(&mut row.line, level, theme);
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
    let mut lines: Vec<Line<'static>> = rows.iter().map(|row| row.line.clone()).collect();
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
        for (offset, row) in rows.iter().enumerate() {
            let Some(y) = u16::try_from(offset)
                .ok()
                .and_then(|o| area.y.checked_add(o))
            else {
                break;
            };
            if y >= area.bottom() {
                break;
            }
            let in_front = row.index.is_some() && row.index == focused;
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
