//! The rail and the full sidebar's inbox: rows by need, the root filter, the edge mark.
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use yi_tui::colors::{Theme, accent_rgb, name_tile, tile_style_at};

use crate::app::App;
use crate::avatar::Placement;
use crate::model::{Mode, SessionId, SessionRow, SessionStatus, SidebarMode, Zone, now_ms};
use crate::render::{NAME_WIDTH, status_style};
use yi_tui::orb::OrbState;
use yi_tui::port::SessionPort;
use yi_types::subagent::{ChildFlag, ChildStatus, ChildUpdate};

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

fn avatar_at(
    col: u16,
    (cols, rows): (u16, u16),
    key: &str,
    hue: usize,
    state: Option<OrbState>,
) -> Option<Placement> {
    Some(Placement {
        col,
        row: 0,
        cols,
        rows,
        key: key.to_owned(),
        seed: key.to_owned(),
        accent: accent_rgb(hue),
        state,
    })
}

fn motion_of(app: &App, id: &SessionId, row: &SessionRow) -> Option<OrbState> {
    let live = app.state.chat(id).and_then(|chat| chat.app.orb_state());
    match row.status {
        SessionStatus::Working => live.or(Some(OrbState::Working)),
        SessionStatus::Blocked => Some(OrbState::Listening),
        SessionStatus::DoneUnseen | SessionStatus::Idle | SessionStatus::Unknown => None,
    }
}

fn detail(app: &App, id: &SessionId, row: &SessionRow, now: u64) -> String {
    let chat = app.state.chat(id);
    let doing = match row.status {
        SessionStatus::Blocked => "needs your answer".to_owned(),
        SessionStatus::DoneUnseen => "done".to_owned(),
        SessionStatus::Working => chat
            .and_then(|chat| chat.app.working_label())
            .unwrap_or_else(|| "working".to_owned()),
        SessionStatus::Idle | SessionStatus::Unknown => "idle".to_owned(),
    };
    let mut parts = vec![doing];
    if chat.is_some_and(|chat| chat.app.gate_red()) {
        parts.insert(0, "gate red".to_owned());
    }
    if let Some(todos) = chat.and_then(|chat| chat.port.todo_list()) {
        let progress = todos.progress();
        if progress.total > 0 {
            parts.push(format!("{}/{}", progress.done, progress.total));
        }
    }
    let claimed = chat
        .and_then(|chat| chat.port.claims(false))
        .map_or(0, |claims| {
            claims.iter().filter(|c| c.observed.is_none()).count()
        });
    if claimed > 0 {
        parts.push(format!("{claimed} claimed"));
    }
    let then = row.recency();
    if then > 0 {
        let secs = now.saturating_sub(then) / 1000;
        parts.push(match secs {
            0..60 => format!("{secs}s"),
            60..3600 => format!("{}m", secs / 60),
            3600..86_400 => format!("{}h", secs / 3600),
            _ => format!("{}d", secs / 86_400),
        });
    }
    parts.join(" · ")
}

fn tile_span(app: &App, text: String, hue: usize) -> Span<'static> {
    if app.kitty {
        Span::raw(" ".repeat(text.chars().count()))
    } else {
        Span::styled(text, tile_style_at(hue))
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
        label.width().saturating_sub(3)
    });
    app.state
        .visible_rows()
        .into_iter()
        .filter_map(|index| app.state.order.get(index))
        .filter_map(|id| {
            let row = app.state.sessions.get(id)?;
            let children = app.state.children.get(id).into_iter().flatten().take(3);
            children
                .map(|child| child.name.width())
                .chain(std::iter::once(row.label().width()))
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
    let mut current_section: Option<&str> = None;
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
        let need = app.state.need_of(row);
        let section = if need <= 2 {
            "needs you"
        } else {
            row.status.section()
        };
        if !rail && current_section != Some(section) {
            current_section = Some(section);
            let style = if need <= 2 {
                status_style(theme, SessionStatus::Blocked).add_modifier(Modifier::BOLD)
            } else {
                theme.dim_style()
            };
            rows.push(SidebarRow::plain(
                None,
                Line::styled(format!("  {section}"), style),
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
                avatar: avatar_at(2, (4, 2), &id.0, hue, motion_of(app, id, row)),
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
        let name_style = if is_focused {
            theme.accent_style().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        let tail = if row.attached { "·" } else { " " };
        let icon = spans.pop();
        spans.pop();
        spans.push(Span::styled("   ".to_owned(), on_row(Style::default())));
        spans.push(Span::styled(pad_cells(&label, cols), on_row(name_style)));
        spans.push(Span::styled(" ".to_owned(), on_row(Style::default())));
        spans.extend(icon);
        spans.push(Span::styled(tail.to_owned(), on_row(theme.dim_style())));
        rows.push(SidebarRow {
            index: Some(index),
            line: Line::from(spans),
            avatar: avatar_at(2, (4, 2), &id.0, hue, motion_of(app, id, row)),
        });
        let text: String = detail(app, id, row, now)
            .chars()
            .take(cols.saturating_add(3))
            .collect();
        rows.push(SidebarRow::plain(
            Some(index),
            Line::from(vec![
                Span::styled(" ".repeat(6), on_row(Style::default())),
                Span::styled(text, on_row(theme.dim_style())),
            ]),
        ));
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

/// `text` cut and padded to exactly `cols` terminal cells, so a wide character never
/// pushes the glyph after it out of its column.
fn pad_cells(text: &str, cols: usize) -> String {
    let mut used = 0_usize;
    let mut out: String = text
        .chars()
        .take_while(|c| {
            used = used.saturating_add(c.width().unwrap_or(0));
            used <= cols
        })
        .collect();
    out.push_str(&" ".repeat(cols.saturating_sub(out.width())));
    out
}

/// A child's mark in the session vocabulary: the states a session has too take its glyph
/// and colour, so `?` means needs you on both and `✕` is left to mean failed.
fn child_glyph(child: &ChildUpdate, theme: &Theme) -> (&'static str, Style) {
    let status = match (&child.status, &child.flag) {
        (ChildStatus::Error, _) => return ("✕", Style::default().fg(theme.error)),
        (ChildStatus::Running, Some(ChildFlag::Stuck { .. })) => {
            return ("!", Style::default().fg(theme.warning));
        }
        (ChildStatus::Running, Some(ChildFlag::NeedsYou { .. })) => SessionStatus::Blocked,
        (ChildStatus::Completed, _) => SessionStatus::Idle,
        (ChildStatus::Running | ChildStatus::Other(_), _) => SessionStatus::Working,
    };
    (status.glyph(), status_style(theme, status))
}

/// Children hang off their session on `├`/`└` at column 2, with the glyph in the
/// session glyph column: 7 on the rail, `cols + 8` in the full sidebar.
fn child_rows(
    app: &App,
    id: &SessionId,
    theme: &Theme,
    rail: bool,
    cols: usize,
) -> Vec<SidebarRow> {
    let children: Vec<&ChildUpdate> = app
        .state
        .children
        .get(id)
        .into_iter()
        .flatten()
        .take(3)
        .collect();
    let last = children.len().saturating_sub(1);
    let mut rows = Vec::new();
    for (index, child) in children.into_iter().enumerate() {
        let (glyph, style) = child_glyph(child, theme);
        let connector = if index == last { "  └" } else { "  ├" };
        let hue = app.accent_of(child.id.as_str(), &child.name);
        let mut spans = vec![
            Span::styled(connector.to_owned(), theme.dim_style()),
            tile_span(app, name_tile(&child.name), hue),
            Span::raw("  "),
        ];
        if !rail {
            let name: String = child.name.chars().filter(|c| !c.is_control()).collect();
            spans.push(Span::styled(pad_cells(&name, cols), theme.dim_style()));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(glyph.to_owned(), style));
        rows.push(SidebarRow {
            index: None,
            line: Line::from(spans),
            avatar: avatar_at(3, (2, 1), child.id.as_str(), hue, None),
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
