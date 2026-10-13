//! The rail and the full sidebar: pinned sessions, then one foldable group per workspace.
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;
use yi_tui::colors::{Theme, accent_rgb, name_tile, tile_style_at};

use crate::app::App;
use crate::avatar::Placement;
use crate::keys::{self, Action};
use crate::model::{Mode, SessionId, SessionRow, SessionStatus, SidebarMode, Zone, now_ms};
use crate::render::{NAME_WIDTH, status_style};
use yi_tui::orb::OrbState;
use yi_tui::port::SessionPort;
use yi_types::status_mark::StatusMark;
use yi_types::subagent::{ChildFlag, ChildStatus, ChildUpdate};

/// A workspace header's last three cells are its `+`, which starts a session in that root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    New(Option<String>),
    Search,
    Fold(String),
}

pub struct SidebarRow {
    pub index: Option<usize>,
    pub line: Line<'static>,
    pub avatar: Option<Placement>,
    pub control: Option<Control>,
}

impl SidebarRow {
    fn plain(index: Option<usize>, line: Line<'static>) -> Self {
        Self {
            index,
            line,
            avatar: None,
            control: None,
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
        art: crate::avatar::Art::Identicon,
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
    let roots = app
        .state
        .roots()
        .into_iter()
        .map(|root| drawn_width(root.rsplit('/').next().unwrap_or(&root)));
    app.state
        .visible_rows()
        .into_iter()
        .filter_map(|index| app.state.order.get(index))
        .filter_map(|id| {
            let row = app.state.sessions.get(id)?;
            let (children, _) = shown_children(app, id);
            children
                .into_iter()
                .map(|child| drawn_width(&child.name))
                .chain(std::iter::once(drawn_width(&row.label())))
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
        SidebarMode::Rail => 8,
        SidebarMode::Full => {
            // Incident: narrowing as a turn's children cleared re-wrapped every pane mid-turn.
            app.state.sidebar_cols = name_cols(app);
            u16::try_from(app.state.sidebar_cols.saturating_add(8)).unwrap_or(u16::MAX)
        }
    }
}

pub(crate) fn rail_style(app: &App, theme: &Theme) -> ratatui::style::Style {
    if app.state.zone == Zone::Sidebar || matches!(app.state.mode, Mode::Prefix) {
        theme.accent_style()
    } else {
        theme.dim_style()
    }
}

fn flush_right(left: &str, right: &str, width: usize) -> String {
    let room = width.saturating_sub(drawn_width(right));
    format!("{}{right}", pad_cells(left, room))
}

fn control_row(text: String, style: Style, control: Control) -> SidebarRow {
    SidebarRow {
        control: Some(control),
        ..SidebarRow::plain(None, Line::styled(text, style))
    }
}

fn group_header(
    app: &App,
    root: &str,
    rows: &[usize],
    folded: bool,
    width: usize,
    theme: &Theme,
) -> SidebarRow {
    let label = root.rsplit('/').next().unwrap_or(root);
    let mut tail = vec![Span::styled(
        if folded { " ▸" } else { " ▾" }.to_owned(),
        theme.dim_style(),
    )];
    let worst = rows
        .iter()
        .filter_map(|index| app.state.order.get(*index))
        .filter_map(|id| app.state.sessions.get(id))
        .map(|row| row.status)
        .min_by_key(|status| status.need())
        .filter(|status| folded && status.need() <= 1);
    if let Some(worst) = worst {
        tail.push(Span::styled(
            format!(" {}", worst.glyph()),
            status_style(theme, worst),
        ));
    }
    let tail_cols: usize = tail.iter().map(|span| drawn_width(&span.content)).sum();
    let room = width.saturating_sub(tail_cols.saturating_add(4));
    let name = pad_cells(label, drawn_width(label).min(room));
    let pad = room.saturating_sub(drawn_width(&name));
    let mut spans = vec![Span::styled(
        format!(" {name}"),
        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
    )];
    spans.extend(tail);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(Span::styled("  +".to_owned(), theme.dim_style()));
    SidebarRow {
        control: Some(Control::Fold(root.to_owned())),
        ..SidebarRow::plain(None, Line::from(spans))
    }
}

pub fn sidebar_lines(app: &App, theme: &Theme, height: u16) -> Vec<SidebarRow> {
    let mut rows = Vec::new();
    let rail = app.state.sidebar == SidebarMode::Rail;
    let mut beyond = 0_usize;
    let focused = app.state.focused_session();
    let now = now_ms();
    let mut slot = 0_usize;
    let cols = name_cols(app);
    let width = cols.saturating_add(8);
    if !rail {
        let chord = |action| keys::chord_for(action, app.cmd_hints);
        let text = Style::default().fg(theme.text);
        let new = flush_right(" + New session", chord(Action::NewSession), width);
        rows.push(control_row(new, text, Control::New(None)));
        let search = flush_right(" ⌕ Search", chord(Action::Navigator), width);
        rows.push(control_row(search, text, Control::Search));
    }
    for group in app.state.groups() {
        if rail {
            if group.rows.is_empty() || slot >= RAIL_CAP {
                beyond = beyond.saturating_add(group.rows.len());
                continue;
            }
            if slot > 0 {
                rows.push(SidebarRow::plain(None, Line::raw("")));
            }
        } else {
            match &group.root {
                None if group.rows.is_empty() => continue,
                None => rows.push(SidebarRow::plain(
                    None,
                    Line::styled(" PINNED", theme.dim_style().add_modifier(Modifier::BOLD)),
                )),
                Some(root) => rows.push(group_header(
                    app,
                    root,
                    &group.rows,
                    group.folded,
                    width,
                    theme,
                )),
            }
        }
        if group.folded {
            continue;
        }
        for index in group.rows {
            let Some(id) = app.state.order.get(index) else {
                continue;
            };
            let Some(row) = app.state.sessions.get(id) else {
                continue;
            };
            let is_focused = focused.as_ref() == Some(id);
            let is_cursor = index == app.state.selected && app.state.zone == Zone::Sidebar;
            slot = slot.saturating_add(1);
            if rail && slot > RAIL_CAP {
                beyond = beyond.saturating_add(1);
                continue;
            }
            rows.extend(session_rows(
                app,
                theme,
                (index, id, row),
                (is_focused, is_cursor),
                (slot, cols, now),
            ));
        }
    }
    if beyond > 0 {
        rows.push(SidebarRow::plain(
            None,
            Line::styled(format!("  +{beyond}"), theme.dim_style()),
        ));
    }
    window_rows(rows, height, app.state.selected, theme)
}

fn session_rows(
    app: &App,
    theme: &Theme,
    (index, id, row): (usize, &SessionId, &SessionRow),
    (is_focused, is_cursor): (bool, bool),
    (slot, cols, now): (usize, usize, u64),
) -> Vec<SidebarRow> {
    let rail = app.state.sidebar == SidebarMode::Rail;
    let mut rows = Vec::new();
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
                Span::styled("  ".to_owned(), on_row(Style::default())),
                Span::styled(
                    row.status.glyph().to_owned(),
                    on_row(status_style(theme, row.status)),
                ),
            ]),
            avatar: avatar_at(2, (4, 2), &id.0, hue, motion_of(app, id, row)),
            control: None,
        });
        rows.push(SidebarRow::plain(
            Some(index),
            Line::from(Span::styled(" ".repeat(7), on_row(Style::default()))),
        ));
        rows.extend(child_rows(app, id, theme, true, cols));
        return rows;
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
    spans.push(Span::styled("  ".to_owned(), on_row(Style::default())));
    spans.push(Span::styled(pad_cells(&label, cols), on_row(name_style)));
    spans.push(Span::styled(" ".to_owned(), on_row(Style::default())));
    spans.extend(icon);
    spans.push(Span::styled(tail.to_owned(), on_row(theme.dim_style())));
    rows.push(SidebarRow {
        index: Some(index),
        line: Line::from(spans),
        avatar: avatar_at(2, (4, 2), &id.0, hue, motion_of(app, id, row)),
        control: None,
    });
    let text: String = detail(app, id, row, now)
        .chars()
        .take(cols.saturating_add(2))
        .collect();
    rows.push(SidebarRow::plain(
        Some(index),
        Line::from(vec![
            Span::styled(" ".repeat(6), on_row(Style::default())),
            Span::styled(text, on_row(theme.dim_style())),
        ]),
    ));
    rows.extend(child_rows(app, id, theme, false, cols));
    rows
}

/// The cells `text` takes once its control characters are dropped, as `pad_cells` draws it.
fn drawn_width(text: &str) -> usize {
    text.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .width()
}

/// `text` without control characters, cut and padded to exactly `cols` drawn cells: `⚠️`
/// is 1 + 0 cells a character at a time but 2 as a string, so it measures the string.
fn pad_cells(text: &str, cols: usize) -> String {
    let mut out = String::new();
    for c in text.chars().filter(|c| !c.is_control()) {
        out.push(c);
        if out.width() > cols {
            out.pop();
            break;
        }
    }
    out.push_str(&" ".repeat(cols.saturating_sub(out.width())));
    out
}

/// What a child's row says, in the session vocabulary.
fn child_mark(child: &ChildUpdate) -> StatusMark {
    match (&child.status, &child.flag) {
        (ChildStatus::Error, _) => StatusMark::Failed,
        (ChildStatus::Running, Some(ChildFlag::NeedsYou { .. })) => StatusMark::NeedsYou,
        (ChildStatus::Running, Some(ChildFlag::Stuck { .. })) => StatusMark::Stuck,
        (ChildStatus::Completed, _) => StatusMark::Idle,
        (ChildStatus::Running | ChildStatus::Other(_), _) => StatusMark::Working,
    }
}

/// A child's glyph takes the colour a session with the same mark wears.
fn mark_style(mark: StatusMark, theme: &Theme) -> Style {
    match mark {
        StatusMark::NeedsYou => status_style(theme, SessionStatus::Blocked),
        StatusMark::Working => status_style(theme, SessionStatus::Working),
        StatusMark::DoneUnseen => status_style(theme, SessionStatus::DoneUnseen),
        StatusMark::Idle => status_style(theme, SessionStatus::Idle),
        StatusMark::Unknown => status_style(theme, SessionStatus::Unknown),
        StatusMark::Failed => Style::default().fg(theme.error),
        StatusMark::Stuck => Style::default().fg(theme.warning),
    }
}

/// The children worth a row, most in need first, and how many more there are: a child that
/// needs you is never hidden behind three finished ones.
fn shown_children<'a>(app: &'a App, id: &SessionId) -> (Vec<&'a ChildUpdate>, usize) {
    let mut children: Vec<&ChildUpdate> =
        app.state.children.get(id).into_iter().flatten().collect();
    children.sort_by_key(|child| match child_mark(child) {
        StatusMark::NeedsYou => 0,
        StatusMark::Failed => 1,
        StatusMark::Stuck => 2,
        StatusMark::Working | StatusMark::DoneUnseen | StatusMark::Unknown => 3,
        StatusMark::Idle => 4,
    });
    let more = children.len().saturating_sub(CHILD_ROWS);
    children.truncate(CHILD_ROWS);
    (children, more)
}

const CHILD_ROWS: usize = 3;

/// Children hang off their session on `├`/`└` at column 2, the glyph in the session glyph
/// column (6 on the rail, `cols + 7` in full); past three, the last row counts the rest.
fn child_rows(
    app: &App,
    id: &SessionId,
    theme: &Theme,
    rail: bool,
    cols: usize,
) -> Vec<SidebarRow> {
    let (children, more) = shown_children(app, id);
    let last = (more == 0).then(|| children.len().saturating_sub(1));
    let mut rows = Vec::new();
    for (index, child) in children.into_iter().enumerate() {
        let mark = child_mark(child);
        let connector = if Some(index) == last {
            "  └"
        } else {
            "  ├"
        };
        let hue = app.accent_of(child.id.as_str(), &child.name);
        let mut spans = vec![
            Span::styled(connector.to_owned(), theme.dim_style()),
            tile_span(app, name_tile(&child.name), hue),
            Span::raw(" "),
        ];
        if !rail {
            spans.push(Span::styled(
                pad_cells(&child.name, cols),
                theme.dim_style(),
            ));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            mark.glyph().to_owned(),
            mark_style(mark, theme),
        ));
        rows.push(SidebarRow {
            index: None,
            line: Line::from(spans),
            avatar: avatar_at(3, (2, 1), child.id.as_str(), hue, None),
            control: None,
        });
    }
    if more > 0 {
        let label = if rail {
            format!("  └ +{more}")
        } else {
            format!("  └ +{more} more")
        };
        rows.push(SidebarRow::plain(
            None,
            Line::styled(label, theme.dim_style()),
        ));
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

pub(crate) fn render_sidebar(app: &App, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let rows = sidebar_lines(app, theme, area.height);
    let lines: Vec<Line<'static>> = rows.iter().map(|row| row.line.clone()).collect();
    frame.render_widget(Paragraph::new(lines), area);
    if app.state.sidebar == SidebarMode::Rail {
        let style = rail_style(app, theme);
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
