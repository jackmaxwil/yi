//! compute_view(&mut) then render(&): geometry and cache mutation first,
//! drawing reads only; ViewState doubles as the mouse hit-test table.

use std::collections::HashMap;

use crate::model::SessionId;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use yi_tui::colors::Theme;
use yi_tui::diffview::{self, DiffBudget};

use crate::app::App;
use crate::keys;
use crate::layout::{PaneId, SplitBorder};
use crate::model::{Link, Mode, PaneContent, SessionStatus, Zone};

const SIDEBAR_WIDTH: u16 = 26;
const COMPOSER_HEIGHT: u16 = 3;

pub struct PaneView {
    pub id: PaneId,
    pub rect: Rect,
    pub focused: bool,
    pub title: String,
    pub lines: Vec<Line<'static>>,
}

/// Mouse hit-test table, refreshed by every compute_view.
pub struct Hits {
    pub sidebar_width: u16,
    /// (screen row, index into `order`) for each session line.
    pub sidebar_rows: Vec<(u16, usize)>,
    pub panes: Vec<(PaneId, Rect)>,
    pub splits: Vec<SplitBorder>,
    pub tabs: Vec<(Rect, usize)>,
}

pub struct ViewState {
    pub sidebar: Rect,
    pub tab_bar: Option<Rect>,
    pub panes_area: Rect,
    pub panes: Vec<PaneView>,
    pub split_borders: Vec<SplitBorder>,
    pub ask: Option<Rect>,
    pub composer: Rect,
    pub status: Rect,
}

fn split_off_bottom(area: Rect, height: u16) -> (Rect, Rect) {
    let height = height.min(area.height);
    let top = Rect {
        height: area.height.saturating_sub(height),
        ..area
    };
    let bottom = Rect {
        y: area.y.saturating_add(top.height),
        height,
        ..area
    };
    (top, bottom)
}

fn split_off_top(area: Rect, height: u16) -> (Rect, Rect) {
    let height = height.min(area.height);
    let top = Rect { height, ..area };
    let rest = Rect {
        y: area.y.saturating_add(height),
        height: area.height.saturating_sub(height),
        ..area
    };
    (top, rest)
}

pub fn compute_view(app: &mut App, area: Rect, theme: &Theme) -> ViewState {
    let sidebar_width = if area.width >= 50 && !app.state.sidebar_hidden {
        SIDEBAR_WIDTH
    } else {
        0
    };
    let sidebar = Rect {
        width: sidebar_width,
        ..area
    };
    let main = Rect {
        x: area.x.saturating_add(sidebar_width),
        width: area.width.saturating_sub(sidebar_width),
        ..area
    };
    let (main, status) = split_off_bottom(main, 1);
    let (main, composer) = split_off_bottom(main, COMPOSER_HEIGHT);
    let (transcript_area, ask) = if app.state.ask.is_some() {
        let (rest, bar) = split_off_bottom(main, 3);
        (rest, Some(bar))
    } else {
        (main, None)
    };
    let (tab_bar, panes_area) = if app.state.tabs.len() > 1 {
        let (bar, rest) = split_off_top(transcript_area, 1);
        (Some(bar), rest)
    } else {
        (None, transcript_area)
    };

    let mut pane_rects = Vec::new();
    let mut split_borders = Vec::new();
    if let Some(tab) = app.state.tab() {
        if tab.zoomed {
            pane_rects.push(crate::layout::PaneRect {
                id: tab.layout.focused(),
                rect: panes_area,
                focused: true,
            });
        } else {
            pane_rects = tab.layout.panes(panes_area);
            split_borders = tab.layout.splits(panes_area);
        }
    }

    let panes_zone = app.state.zone == Zone::Panes;
    let mut panes = Vec::new();
    for pane_rect in pane_rects {
        let inner = pane_rect.rect.inner(ratatui::layout::Margin::new(1, 1));
        let diffs = &app.state.diffs;
        let (title, lines) = match app.state.panes.get_mut(&pane_rect.id) {
            Some(pane) => pane_view_content(pane, diffs, inner, theme),
            None => ("empty".to_owned(), Vec::new()),
        };
        panes.push(PaneView {
            id: pane_rect.id,
            rect: pane_rect.rect,
            focused: pane_rect.focused && panes_zone,
            title,
            lines,
        });
    }

    let sidebar_rows = sidebar_lines(app, theme)
        .iter()
        .enumerate()
        .filter_map(|(offset, (index, _))| {
            let y = sidebar.y.checked_add(u16::try_from(offset).ok()?)?;
            index.map(|index| (y, index))
        })
        .collect();
    let mut tab_hits = Vec::new();
    if let Some(bar) = tab_bar {
        let mut x = bar.x;
        // Invariant: hit widths come from the label builder the bar draws with.
        for (index, tab) in app.state.tabs.iter().enumerate() {
            let width = u16::try_from(tab_label(index, tab.zoomed).chars().count()).unwrap_or(3);
            tab_hits.push((Rect::new(x, bar.y, width, 1), index));
            x = x.saturating_add(width);
        }
    }
    app.hits = Some(Hits {
        sidebar_width,
        sidebar_rows,
        panes: panes.iter().map(|pane| (pane.id, pane.rect)).collect(),
        splits: split_borders.clone(),
        tabs: tab_hits,
    });

    ViewState {
        sidebar,
        tab_bar,
        panes_area,
        panes,
        split_borders,
        ask,
        composer,
        status,
    }
}

fn pane_view_content(
    pane: &mut crate::model::Pane,
    diffs: &std::collections::BTreeMap<SessionId, crate::model::SessionDiff>,
    inner: Rect,
    theme: &Theme,
) -> (String, Vec<Line<'static>>) {
    let visible = usize::from(inner.height);
    match &mut pane.content {
        PaneContent::Session {
            session,
            transcript,
        } => {
            transcript.set_width(usize::from(inner.width));
            let all = transcript.lines(theme);
            let max_scroll = all.len().saturating_sub(visible);
            if pane.scroll_from_bottom > max_scroll {
                pane.scroll_from_bottom = max_scroll;
            }
            let lines = window(all, pane.scroll_from_bottom, visible);
            let title = session.as_ref().map_or_else(
                || "no session".to_owned(),
                |id| id.0.chars().take(14).collect(),
            );
            (title, lines)
        }
        PaneContent::Markdown { path, source } => {
            let all = yi_tui::markdown::render(source, usize::from(inner.width), theme);
            let lines = window(all, pane.scroll_from_bottom, visible);
            (short_path(path), lines)
        }
        PaneContent::Diff { path, source } => {
            let all = diffview::render(source, usize::from(inner.width), theme, DiffBudget::FULL);
            let lines = window(all, pane.scroll_from_bottom, visible);
            (short_path(path), lines)
        }
        PaneContent::SessionDiff { session } => {
            let short: String = session.0.chars().take(8).collect();
            let (title, all) = match diffs.get(session).filter(|diff| !diff.files.is_empty()) {
                Some(diff) => {
                    let (added, removed) = diff.totals();
                    let joined = diff
                        .files
                        .iter()
                        .map(|(_, file)| file.patch.as_str())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let files = diff.files.len();
                    let noun = if files == 1 { "file" } else { "files" };
                    let loose = diff.files.iter().filter(|(_, file)| !file.tracked).count();
                    let loose = if loose == 0 {
                        String::new()
                    } else {
                        format!(" · {loose} untracked")
                    };
                    (
                        format!("Δ {short} · {files} {noun} · +{added} −{removed}{loose}"),
                        diffview::render(
                            &joined,
                            usize::from(inner.width),
                            theme,
                            DiffBudget::FULL,
                        ),
                    )
                }
                None => (
                    format!("Δ {short}"),
                    vec![Line::styled("no edits yet this session", theme.dim_style())],
                ),
            };
            let lines = window(all, pane.scroll_from_bottom, visible);
            (title, lines)
        }
        PaneContent::Notebook { session, cells } => {
            let title = session.as_ref().map_or_else(
                || "notebook".to_owned(),
                |id| format!("nb:{}", id.0.chars().take(11).collect::<String>()),
            );
            let all = notebook_lines(cells, usize::from(inner.width), theme);
            let lines = window(all, pane.scroll_from_bottom, visible);
            (title, lines)
        }
    }
}

fn notebook_lines(
    cells: &[crate::model::NbCell],
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    if cells.is_empty() {
        out.push(Line::styled(
            "no kernel cells yet — they appear as the agent computes",
            theme.dim_style(),
        ));
        return out;
    }
    for (index, cell) in cells.iter().enumerate() {
        let marker = if cell.running { "◐" } else { "●" };
        out.push(Line::styled(
            format!("{marker} In[{}]", index.saturating_add(1)),
            theme.accent_style(),
        ));
        for code_line in cell.code.lines().take(12) {
            let clipped: String = code_line.chars().take(width.max(1)).collect();
            out.push(Line::styled(format!("  {clipped}"), theme.muted_style()));
        }
        for stream_line in cell.stdout.lines().take(20) {
            let clipped: String = stream_line.chars().take(width.max(1)).collect();
            out.push(Line::styled(clipped, Style::default().fg(theme.text)));
        }
        if let Some(result) = &cell.result {
            for result_line in result.lines().take(10) {
                let clipped: String = result_line.chars().take(width.max(1)).collect();
                out.push(Line::styled(clipped, Style::default().fg(theme.text)));
            }
        }
        if let Some(error) = &cell.error {
            let clipped: String = error.chars().take(width.max(1)).collect();
            out.push(Line::styled(clipped, Style::default().fg(theme.error)));
        }
        for image in &cell.images {
            let kb = image.len().saturating_mul(3) / 4 / 1024;
            out.push(Line::styled(
                format!("▲ [image {kb} KB png]"),
                theme.accent_style(),
            ));
        }
        out.push(Line::default());
    }
    out
}

fn window(all: Vec<Line<'static>>, from_bottom: usize, visible: usize) -> Vec<Line<'static>> {
    let end = all.len().saturating_sub(from_bottom);
    let start = end.saturating_sub(visible);
    all.get(start..end)
        .map(<[Line<'static>]>::to_vec)
        .unwrap_or_default()
}

fn short_path(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

fn status_style(theme: &Theme, status: SessionStatus) -> Style {
    match status {
        SessionStatus::Blocked => Style::default().fg(theme.error),
        SessionStatus::Working => Style::default().fg(theme.warning),
        SessionStatus::DoneUnseen => Style::default()
            .fg(theme.success)
            .add_modifier(Modifier::BOLD),
        SessionStatus::Idle => theme.muted_style(),
        SessionStatus::Unknown => theme.dim_style(),
    }
}

pub fn render(app: &App, frame: &mut Frame<'_>, view: &ViewState, theme: &Theme) {
    if view.sidebar.width > 0 {
        render_sidebar(app, frame, view.sidebar, theme);
    }
    if let Some(bar) = view.tab_bar {
        render_tab_bar(app, frame, bar, theme);
    }
    for pane in &view.panes {
        let inner = pane.rect.inner(ratatui::layout::Margin::new(1, 1));
        frame.render_widget(Paragraph::new(pane.lines.clone()), inner);
    }
    render_borders(app, frame, view, theme);
    if let Some(area) = view.ask {
        render_ask(app, frame, area, theme);
    }
    frame.render_widget(&app.composer, view.composer);
    render_status(app, frame, view.status, theme);
    if matches!(app.state.mode, Mode::Navigator { .. }) {
        render_navigator(app, frame, view, theme);
    }
}

/// Border cells with edge-bit union, so shared pane edges resolve to real
/// junctions instead of doubled lines.
fn render_borders(app: &App, frame: &mut Frame<'_>, view: &ViewState, theme: &Theme) {
    #[derive(Default, Clone, Copy)]
    struct EdgeCell {
        up: bool,
        down: bool,
        left: bool,
        right: bool,
        focused: bool,
    }
    let mut raster: HashMap<(u16, u16), EdgeCell> = HashMap::new();
    for pane in &view.panes {
        let r = pane.rect;
        if r.width < 2 || r.height < 2 {
            continue;
        }
        let right = r.x.saturating_add(r.width).saturating_sub(1);
        let bottom = r.y.saturating_add(r.height).saturating_sub(1);
        for x in r.x..=right {
            let left_arm = x > r.x;
            let right_arm = x < right;
            for y in [r.y, bottom] {
                let cell = raster.entry((x, y)).or_default();
                cell.left |= left_arm;
                cell.right |= right_arm;
                cell.focused |= pane.focused;
            }
        }
        for y in r.y..=bottom {
            let up_arm = y > r.y;
            let down_arm = y < bottom;
            for x in [r.x, right] {
                let cell = raster.entry((x, y)).or_default();
                cell.up |= up_arm;
                cell.down |= down_arm;
                cell.focused |= pane.focused;
            }
        }
    }
    let buffer = frame.buffer_mut();
    for ((x, y), cell) in &raster {
        let symbol = glyph(cell.up, cell.down, cell.left, cell.right);
        if let Some(target) = buffer.cell_mut((*x, *y)) {
            target.set_symbol(symbol);
            target.set_style(if cell.focused {
                theme.accent_style()
            } else {
                theme.dim_style()
            });
        }
    }
    // Titles overwrite the top border after the raster settles.
    for pane in &view.panes {
        let r = pane.rect;
        if r.width < 8 {
            continue;
        }
        let status = app
            .state
            .panes
            .get(&pane.id)
            .and_then(|p| p.session())
            .and_then(|s| app.state.sessions.get(s))
            .map_or(SessionStatus::Unknown, |row| row.status);
        let text = format!(" {} {} ", status.glyph(), pane.title);
        let max = r.width.saturating_sub(4);
        let text: String = text.chars().take(usize::from(max)).collect();
        let style = if pane.focused {
            theme.accent_style().add_modifier(Modifier::BOLD)
        } else {
            theme.dim_style()
        };
        let span = Span::styled(text, style);
        buffer.set_span(r.x.saturating_add(2), r.y, &span, max);
    }
}

fn glyph(up: bool, down: bool, left: bool, right: bool) -> &'static str {
    match (up, down, left, right) {
        (true, true, true, true) => "┼",
        (true, true, true, false) => "┤",
        (true, true, false, true) => "├",
        (true, true, false, false) | (true, false, false, false) | (false, true, false, false) => {
            "│"
        }
        (true, false, true, true) => "┴",
        (false, true, true, true) => "┬",
        (false, false, true, true) | (false, false, true, false) | (false, false, false, true) => {
            "─"
        }
        (true, false, true, false) => "┘",
        (true, false, false, true) => "└",
        (false, true, true, false) => "┐",
        (false, true, false, true) => "┌",
        (false, false, false, false) => " ",
    }
}

fn tab_label(index: usize, zoomed: bool) -> String {
    let zoom = if zoomed { "Z" } else { "" };
    format!(" {}{zoom} ", index.saturating_add(1))
}

fn render_tab_bar(app: &App, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let mut spans = Vec::new();
    for (index, tab) in app.state.tabs.iter().enumerate() {
        let label = tab_label(index, tab.zoomed);
        let style = if index == app.state.active_tab {
            theme.accent_style().add_modifier(Modifier::BOLD)
        } else {
            theme.dim_style()
        };
        spans.push(Span::styled(label, style));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Sidebar lines paired with the `order` index a row stands for; root
/// headers carry None. One builder serves rendering and hit-testing.
fn sidebar_lines(app: &App, theme: &Theme) -> Vec<(Option<usize>, Line<'static>)> {
    let mut lines = Vec::new();
    let multi_root = app.state.roots().len() > 1;
    let mut current_root: Option<&str> = None;
    for (index, id) in app.state.order.iter().enumerate() {
        let Some(row) = app.state.sessions.get(id) else {
            continue;
        };
        if multi_root && current_root != Some(row.root.as_str()) {
            current_root = Some(row.root.as_str());
            let label = row.root.rsplit('/').next().unwrap_or(&row.root);
            lines.push((
                None,
                Line::styled(
                    format!(" {label}"),
                    theme.accent_style().add_modifier(Modifier::BOLD),
                ),
            ));
        }
        let selected = index == app.state.selected && app.state.zone == Zone::Sidebar;
        let marker = if selected { "▸" } else { " " };
        let short: String = id.0.chars().take(16).collect();
        let mut spans = vec![
            Span::styled(marker.to_owned(), theme.accent_style()),
            Span::styled(
                format!("{} ", row.status.glyph()),
                status_style(theme, row.status),
            ),
            Span::styled(short, Style::default().fg(theme.text)),
        ];
        if row.attached {
            spans.push(Span::styled(" ·", theme.dim_style()));
        }
        lines.push((Some(index), Line::from(spans)));
    }
    lines
}

fn render_sidebar(app: &App, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let mut lines: Vec<Line<'static>> = sidebar_lines(app, theme)
        .into_iter()
        .map(|(_, line)| line)
        .collect();
    if lines.is_empty() {
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
}

fn render_ask(app: &App, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let Some(ask) = &app.state.ask else { return };
    let title = Line::from(vec![
        Span::styled("⚠ ", Style::default().fg(theme.warning)),
        Span::styled(
            ask.params.title.clone(),
            Style::default()
                .fg(theme.warning)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    let detail = ask
        .params
        .description
        .clone()
        .unwrap_or_else(|| "the agent asks for permission".to_owned());
    let key_hints = Line::from(vec![
        Span::styled("a", theme.accent_style()),
        Span::styled(" allow once  ", theme.dim_style()),
        Span::styled("A", theme.accent_style()),
        Span::styled(" always  ", theme.dim_style()),
        Span::styled("r", theme.accent_style()),
        Span::styled(" reject", theme.dim_style()),
    ]);
    frame.render_widget(
        Paragraph::new(vec![
            title,
            Line::styled(detail, theme.muted_style()),
            key_hints,
        ]),
        area,
    );
}

fn render_navigator(app: &App, frame: &mut Frame<'_>, view: &ViewState, theme: &Theme) {
    let Mode::Navigator { query, selected } = &app.state.mode else {
        return;
    };
    let area = view.panes_area;
    let width = area.width.saturating_sub(8).clamp(20, 60).min(area.width);
    let height = area.height.saturating_sub(4).clamp(4, 14).min(area.height);
    let popup = Rect {
        x: area.x.saturating_add(area.width.saturating_sub(width) / 2),
        y: area.y.saturating_add(1),
        width,
        height,
    };
    frame.render_widget(ratatui::widgets::Clear, popup);
    let mut lines = vec![Line::from(vec![
        Span::styled("find ", theme.dim_style()),
        Span::styled(query.clone(), theme.accent_style()),
        Span::styled("▌", theme.accent_style()),
    ])];
    let matches = app.navigator_matches(query);
    let visible = usize::from(height.saturating_sub(2));
    let picked_index = (*selected).min(matches.len().saturating_sub(1));
    for (index, id) in matches.iter().take(visible).enumerate() {
        let row_status = app
            .state
            .sessions
            .get(id)
            .map_or(SessionStatus::Unknown, |row| row.status);
        let picked = index == picked_index;
        let marker = if picked { "▸ " } else { "  " };
        lines.push(Line::from(vec![
            Span::styled(marker, theme.accent_style()),
            Span::styled(
                format!("{} ", row_status.glyph()),
                status_style(theme, row_status),
            ),
            Span::styled(
                id.0.chars().take(30).collect::<String>(),
                if picked {
                    theme.accent_style().add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text)
                },
            ),
        ]));
    }
    if matches.is_empty() {
        lines.push(Line::styled("  no matches", theme.dim_style()));
    }
    frame.render_widget(Paragraph::new(lines), popup);
}

fn render_status(app: &App, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    if matches!(app.state.mode, Mode::Prefix) {
        let spans = vec![
            Span::styled(
                " PREFIX ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::REVERSED),
            ),
            Span::styled(
                format!(" {}", keys::hint(true, app.cmd_hints)),
                theme.dim_style(),
            ),
        ];
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
        return;
    }
    let link = match &app.state.link {
        Link::Connecting => Span::styled("connecting…", theme.dim_style()),
        Link::Connected => Span::styled("● connected", Style::default().fg(theme.success)),
        Link::Disconnected { reason } => {
            let short: String = reason.chars().take(28).collect();
            Span::styled(
                format!("✕ disconnected: {short}"),
                Style::default().fg(theme.error),
            )
        }
    };
    // Invariant: the note renders first so a long link reason can never
    // push the actionable part off the row.
    let mut spans = Vec::new();
    if let Some(note) = &app.state.status_note {
        spans.push(Span::styled(
            format!("{note}  "),
            Style::default().fg(theme.warning),
        ));
    }
    spans.push(link);
    if let Some(id) = &app.state.focused_session() {
        let short: String = id.0.chars().take(16).collect();
        spans.push(Span::styled(format!("  {short}"), theme.muted_style()));
    }
    if let Some((used, size)) = app.state.tokens_used
        && size > 0
    {
        spans.push(Span::styled(
            format!("  {}k/{}k", used / 1000, size / 1000),
            theme.dim_style(),
        ));
    }
    if app.state.dropped_frames > 0 {
        spans.push(Span::styled(
            format!("  dropped:{}", app.state.dropped_frames),
            theme.dim_style(),
        ));
    }
    spans.push(Span::styled(
        format!("  {}", keys::hint(false, app.cmd_hints)),
        theme.dim_style(),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
