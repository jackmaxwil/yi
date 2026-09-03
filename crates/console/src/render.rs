//! compute_view(&mut) then render(&): geometry and cache mutation first,
//! drawing reads only; ViewState doubles as the mouse hit-test table.

use std::collections::HashMap;

use crate::model::{SessionId, SessionRow, SidebarMode, now_ms};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use yi_tui::colors::{Theme, name_accent};
use yi_tui::diffview::{self, DiffBudget};
use yi_tui::popup::BottomView;
use yi_tui::status::{StatusInput, working_line};

use crate::app::App;
use crate::keys;
use crate::layout::{PaneId, SplitBorder};
use crate::model::{Link, Mode, PaneContent, SessionStatus, Zone};

const NAME_WIDTH: usize = 16;

pub struct PaneView {
    pub scroll: Option<(usize, usize)>,
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
    /// (screen row, index into `roots()`) for each workspace line.
    pub root_rows: Vec<(u16, usize)>,
    pub panes: Vec<(PaneId, Rect)>,
    pub splits: Vec<SplitBorder>,
    pub tabs: Vec<(Rect, usize)>,
}

pub struct ViewState {
    pub sidebar: Rect,
    pub roots: Rect,
    pub strip: Option<Rect>,
    pub editor_cursor: Option<(u16, u16)>,
    pub tab_bar: Option<Rect>,
    pub panes_area: Rect,
    pub panes: Vec<PaneView>,
    pub split_borders: Vec<SplitBorder>,
    pub ask: Option<Rect>,
    pub composer: Rect,
    pub status: Rect,
    pub framed: bool,
}

fn pane_margin(framed: bool) -> ratatui::layout::Margin {
    ratatui::layout::Margin::new(1, u16::from(framed))
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

fn composer_height(app: &mut App, width: u16, theme: &Theme) -> u16 {
    let border = if app.state.zone == Zone::Panes {
        theme.muted_style()
    } else {
        theme.dim_style()
    };
    app.composer.set_frame(border, theme.dim_style());
    match &app.bottom {
        Some(bottom) => {
            let rows = bottom.popup().lines(usize::from(width), theme).len();
            u16::try_from(rows).unwrap_or(3)
        }
        None => app.composer.desired_height(),
    }
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
    let sidebar_width = if area.width >= 50 {
        app.state.sidebar.width()
    } else {
        0
    };
    let sidebar = Rect {
        width: sidebar_width,
        ..area
    };
    let root_count = app.state.roots().len();
    let roots_height = if sidebar_width > 0 && app.state.sidebar == SidebarMode::Full {
        u16::try_from(root_count.saturating_add(1).min(8)).unwrap_or(8)
    } else {
        0
    };
    let (sidebar, roots_area) = split_off_bottom(sidebar, roots_height);
    let main = Rect {
        x: area.x.saturating_add(sidebar_width),
        width: area.width.saturating_sub(sidebar_width),
        ..area
    };
    let (main, status) = split_off_bottom(main, 1);
    let (main, composer) = split_off_bottom(main, composer_height(app, main.width, theme));
    let (main, strip) = if strip_line(app, theme).is_some() {
        let (rest, row) = split_off_bottom(main, 1);
        (rest, Some(row))
    } else {
        (main, None)
    };
    let (transcript_area, ask) = if app.state.ask.is_some() {
        let (rest, bar) = split_off_bottom(main, 5);
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

    let lone_chat = pane_rects.len() == 1
        && pane_rects
            .first()
            .and_then(|pane_rect| app.state.panes.get(&pane_rect.id))
            .is_some_and(|pane| matches!(pane.content, PaneContent::Session { .. }));
    let framed = !lone_chat || app.state.tab().is_some_and(|tab| tab.zoomed);
    let panes_zone = app.state.zone == Zone::Panes;
    let mut panes = Vec::new();
    let mut editor_cursor = None;
    for pane_rect in pane_rects {
        let inner = pane_rect.rect.inner(pane_margin(framed));
        let diffs = &app.state.diffs;
        let sessions = &app.state.sessions;
        let (title, lines, scroll) = match app.state.panes.get_mut(&pane_rect.id) {
            Some(pane) => pane_view_content(pane, diffs, sessions, inner, theme),
            None => ("empty".to_owned(), Vec::new(), None),
        };
        if pane_rect.focused
            && panes_zone
            && let Some(PaneContent::Editor(editor)) =
                app.state.panes.get(&pane_rect.id).map(|pane| &pane.content)
        {
            editor_cursor = editor_cursor_cell(editor, inner);
        }
        panes.push(PaneView {
            scroll,
            id: pane_rect.id,
            rect: pane_rect.rect,
            focused: pane_rect.focused && panes_zone,
            title,
            lines,
        });
    }

    let sidebar_rows = sidebar_lines(app, theme, sidebar.height)
        .iter()
        .enumerate()
        .filter_map(|(offset, (index, _))| {
            let y = sidebar.y.checked_add(u16::try_from(offset).ok()?)?;
            index.map(|index| (y, index))
        })
        .collect();
    let root_rows = (0..root_count)
        .filter_map(|index| {
            let y = roots_area
                .y
                .checked_add(u16::try_from(index.saturating_add(1)).ok()?)?;
            (y < roots_area.y.saturating_add(roots_area.height)).then_some((y, index))
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
        root_rows,
        panes: panes.iter().map(|pane| (pane.id, pane.rect)).collect(),
        splits: split_borders.clone(),
        tabs: tab_hits,
    });

    ViewState {
        sidebar,
        roots: roots_area,
        strip,
        editor_cursor,
        tab_bar,
        panes_area,
        panes,
        split_borders,
        ask,
        composer,
        status,
        framed,
    }
}

fn label_of(
    sessions: &std::collections::BTreeMap<SessionId, SessionRow>,
    id: &SessionId,
    max: usize,
) -> String {
    sessions
        .get(id)
        .map_or_else(|| id.0.clone(), SessionRow::label)
        .chars()
        .take(max)
        .collect()
}

fn pane_view_content(
    pane: &mut crate::model::Pane,
    diffs: &std::collections::BTreeMap<SessionId, crate::model::SessionDiff>,
    sessions: &std::collections::BTreeMap<SessionId, SessionRow>,
    inner: Rect,
    theme: &Theme,
) -> (String, Vec<Line<'static>>, Option<(usize, usize)>) {
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
            let (lines, scroll) = windowed(all, pane.scroll_from_bottom, visible);
            let title = session.as_ref().map_or_else(
                || "❯ no session".to_owned(),
                |id| format!("❯ {}", label_of(sessions, id, 14)),
            );
            (title, lines, scroll)
        }
        PaneContent::Markdown { path, source } => {
            let all = yi_tui::markdown::render(source, usize::from(inner.width), theme);
            let (lines, scroll) = windowed(all, pane.scroll_from_bottom, visible);
            (format!("¶ {}", short_path(path)), lines, scroll)
        }
        PaneContent::Diff { path, source } => {
            let all = diffview::render(source, usize::from(inner.width), theme, DiffBudget::FULL);
            let (lines, scroll) = windowed(all, pane.scroll_from_bottom, visible);
            (format!("Δ {}", short_path(path)), lines, scroll)
        }
        PaneContent::Editor(editor) => editor_view(editor, inner, theme),
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
            let (lines, scroll) = windowed(all, pane.scroll_from_bottom, visible);
            (title, lines, scroll)
        }
        PaneContent::Notebook { session, cells, .. } => {
            let title = session.as_ref().map_or_else(
                || "▤ notebook".to_owned(),
                |id| format!("▤ nb:{}", label_of(sessions, id, 11)),
            );
            let all = notebook_lines(cells, usize::from(inner.width), theme);
            let (lines, scroll) = windowed(all, pane.scroll_from_bottom, visible);
            (title, lines, scroll)
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

/// The visible slice, plus the total and first-visible row a scroll bar is drawn from.
fn windowed(
    all: Vec<Line<'static>>,
    from_bottom: usize,
    visible: usize,
) -> (Vec<Line<'static>>, Option<(usize, usize)>) {
    let total = all.len();
    let top = total.saturating_sub(from_bottom).saturating_sub(visible);
    let extent = (total > visible).then_some((total, top));
    (window(all, from_bottom, visible), extent)
}

fn window(all: Vec<Line<'static>>, from_bottom: usize, visible: usize) -> Vec<Line<'static>> {
    let end = all.len().saturating_sub(from_bottom);
    let start = end.saturating_sub(visible);
    all.get(start..end)
        .map(<[Line<'static>]>::to_vec)
        .unwrap_or_default()
}

fn editor_cursor_cell(editor: &crate::model::Editor, inner: Rect) -> Option<(u16, u16)> {
    let (row, col) = editor.text.cursor();
    let line = editor.text.lines().get(row)?;
    let prefix: String = line.chars().take(col).collect();
    let x = inner
        .x
        .checked_add(u16::try_from(editor.gutter()).ok()?)?
        .checked_add(u16::try_from(Span::raw(prefix.as_str()).width()).ok()?)?;
    let y = inner
        .y
        .checked_add(u16::try_from(row.checked_sub(editor.scroll_top)?).ok()?)?;
    (x < inner.x.saturating_add(inner.width) && y < inner.y.saturating_add(inner.height))
        .then_some((x, y))
}

fn editor_view(
    editor: &mut crate::model::Editor,
    inner: Rect,
    theme: &Theme,
) -> (String, Vec<Line<'static>>, Option<(usize, usize)>) {
    let height = usize::from(inner.height).max(1);
    let total = editor.text.lines().len();
    let cursor = editor.text.cursor();
    if cursor != editor.last_cursor {
        editor.last_cursor = cursor;
        if cursor.0 < editor.scroll_top {
            editor.scroll_top = cursor.0;
        } else if cursor.0 >= editor.scroll_top.saturating_add(height) {
            editor.scroll_top = cursor.0.saturating_add(1).saturating_sub(height);
        }
    }
    editor.scroll_top = editor.scroll_top.min(total.saturating_sub(1));
    let gutter = editor.gutter();
    let mut lang = editor.primed_lang();
    let selection = editor.text.selection_range();
    let base = Style::default().fg(theme.text);
    let mut out = Vec::new();
    for (index, line) in editor
        .text
        .lines()
        .iter()
        .enumerate()
        .skip(editor.scroll_top)
        .take(height)
    {
        let number = format!(
            "{:>width$} ",
            index.saturating_add(1),
            width = gutter.saturating_sub(1)
        );
        let mut spans = vec![Span::styled(number, theme.dim_style())];
        let selected = selection.and_then(|(start, end)| {
            let from = if index == start.0 { start.1 } else { 0 };
            let to = if index == end.0 {
                end.1
            } else {
                line.chars().count()
            };
            (start.0 <= index && index <= end.0).then_some((from, to))
        });
        match (selected, lang.as_mut()) {
            (Some((from, to)), _) => {
                let head: String = line.chars().take(from).collect();
                let mid: String = line
                    .chars()
                    .skip(from)
                    .take(to.saturating_sub(from))
                    .collect();
                let tail: String = line.chars().skip(to).collect();
                spans.push(Span::styled(head, base));
                spans.push(Span::styled(mid, base.add_modifier(Modifier::REVERSED)));
                spans.push(Span::styled(tail, base));
            }
            (None, Some(lang)) => spans.extend(yi_tui::highlight::spans(line, lang, theme, base)),
            (None, None) => spans.push(Span::styled(line.clone(), base)),
        }
        out.push(Line::from(spans));
    }
    if editor.stale {
        out.truncate(height.saturating_sub(1));
        out.push(Line::styled(
            "file changed on disk · r reload · k keep",
            Style::default().fg(theme.warning),
        ));
    }
    let mark = if editor.dirty { " ●" } else { "" };
    let extent = (total > height).then_some((total, editor.scroll_top));
    (format!("✎ {}{mark}", short_path(&editor.path)), out, extent)
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
    if view.roots.height > 0 {
        render_roots(app, frame, view.roots, theme);
    }
    if let Some(bar) = view.tab_bar {
        render_tab_bar(app, frame, bar, theme);
    }
    let crowd = view.panes.len() > 1;
    for pane in &view.panes {
        let inner = pane.rect.inner(pane_margin(view.framed));
        let body = Paragraph::new(pane.lines.clone());
        let body = if crowd && !pane.focused {
            body.style(Style::default().add_modifier(Modifier::DIM))
        } else {
            body
        };
        frame.render_widget(body, inner);
    }
    if let Some(strip) = view.strip {
        render_strip(app, frame, strip, theme);
    }
    if view.framed {
        render_borders(app, frame, view, theme);
    }
    render_thumbs(frame, view, theme);
    if let Some(area) = view.ask {
        render_ask(app, frame, area, theme);
    }
    let notebook_input = app
        .state
        .focused_pane_id()
        .and_then(|id| app.state.panes.get(&id))
        .and_then(|pane| match &pane.content {
            PaneContent::Notebook { input, .. } => Some(input.as_ref()),
            _ => None,
        });
    match (notebook_input, &app.bottom) {
        (Some(input), _) => frame.render_widget(input, view.composer),
        (None, Some(bottom)) => {
            let lines = bottom
                .popup()
                .lines(usize::from(view.composer.width), theme);
            frame.render_widget(Paragraph::new(lines), view.composer);
        }
        (None, None) => frame.render_widget(&app.composer.textarea, view.composer),
    }
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
            theme
                .accent_style()
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            theme.dim_style()
        };
        let span = Span::styled(text, style);
        buffer.set_span(r.x.saturating_add(2), r.y, &span, max);
    }
}

/// A bar on the right border, sized by the fraction of the content on screen.
fn render_thumbs(frame: &mut Frame<'_>, view: &ViewState, theme: &Theme) {
    let buffer = frame.buffer_mut();
    for pane in &view.panes {
        let Some((total, top)) = pane.scroll else {
            continue;
        };
        let track = usize::from(pane.rect.height).saturating_sub(2);
        if track == 0 || pane.rect.width < 2 || total <= track {
            continue;
        }
        let length = track
            .saturating_mul(track)
            .div_ceil(total)
            .max(1)
            .min(track);
        let travel = track.saturating_sub(length);
        let offset = travel
            .saturating_mul(top)
            .checked_div(total.saturating_sub(track))
            .unwrap_or(0)
            .min(travel);
        let style = if pane.focused {
            theme.accent_style()
        } else {
            theme.dim_style()
        };
        let x = pane
            .rect
            .x
            .saturating_add(pane.rect.width)
            .saturating_sub(1);
        for row in 0..length {
            let Ok(step) = u16::try_from(offset.saturating_add(row)) else {
                break;
            };
            let y = pane.rect.y.saturating_add(1).saturating_add(step);
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_symbol("┃");
                cell.set_style(style);
            }
        }
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
        (true, false, true, false) => "╯",
        (true, false, false, true) => "╰",
        (false, true, true, false) => "╮",
        (false, true, false, true) => "╭",
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
fn age(now: u64, then: u64) -> String {
    if then == 0 {
        return String::new();
    }
    let seconds = now.saturating_sub(then) / 1000;
    match seconds {
        0..60 => "now".to_owned(),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
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

fn sidebar_lines(app: &App, theme: &Theme, height: u16) -> Vec<(Option<usize>, Line<'static>)> {
    let mut rows = Vec::new();
    let rail = app.state.sidebar == SidebarMode::Rail;
    let multi_root = !rail && app.state.roots().len() > 1;
    let mut current_root: Option<&str> = None;
    let focused = app.state.focused_session();
    let now = now_ms();
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
            rows.push((
                None,
                Line::styled(
                    format!(" {label}"),
                    theme.accent_style().add_modifier(Modifier::BOLD),
                ),
            ));
        }
        let is_focused = focused.as_ref() == Some(id);
        let marker = if index == app.state.selected && app.state.zone == Zone::Sidebar {
            "▸"
        } else if is_focused {
            "▎"
        } else {
            " "
        };
        let label = row.label();
        if rail {
            let mut glyph = Style::default().fg(name_accent(&label));
            if is_focused {
                glyph = glyph.add_modifier(Modifier::REVERSED);
            }
            rows.push((
                Some(index),
                Line::from(vec![
                    Span::styled(marker.to_owned(), theme.accent_style()),
                    Span::styled(row.status.glyph().to_owned(), glyph),
                ]),
            ));
            continue;
        }
        let name: String = label.chars().take(NAME_WIDTH).collect();
        let pad = NAME_WIDTH.saturating_sub(name.chars().count());
        let name_style = if is_focused {
            theme.accent_style().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        let tail = if row.attached { "·" } else { "" };
        let mut spans = vec![
            Span::styled(marker.to_owned(), theme.accent_style()),
            Span::styled(
                format!("{} ", row.status.glyph()),
                status_style(theme, row.status),
            ),
            Span::styled(name, name_style),
            Span::styled(
                format!("{}{:>4}{tail}", " ".repeat(pad), age(now, row.recency())),
                theme.dim_style(),
            ),
        ];
        spans.retain(|span| !span.content.is_empty());
        rows.push((Some(index), Line::from(spans)));
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
    }
    let visible = usize::from(height);
    if visible == 0 || rows.len() <= visible {
        return rows;
    }
    let cursor = rows
        .iter()
        .position(|(index, _)| *index == Some(app.state.selected))
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

fn render_roots(app: &App, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
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

fn render_sidebar(app: &App, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let mut lines: Vec<Line<'static>> = sidebar_lines(app, theme, area.height)
        .into_iter()
        .map(|(_, line)| line)
        .collect();
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
}

fn strip_text(app: &App) -> Option<String> {
    let session = app.state.focused_session()?;
    let mut parts = Vec::new();
    if let Some(diff) = app.state.diffs.get(&session)
        && !diff.files.is_empty()
    {
        let names: Vec<String> = diff
            .files
            .iter()
            .rev()
            .take(4)
            .map(|(path, _)| short_path(path))
            .collect();
        let more = diff.files.len().saturating_sub(names.len());
        let tail = if more > 0 {
            format!(" +{more}")
        } else {
            String::new()
        };
        parts.push(format!("touched {}{tail}", names.join(", ")));
    }
    let running = app.state.children.get(&session).map_or(0, |rows| {
        rows.iter()
            .filter(|row| row.status == yi_types::subagent::ChildStatus::Running)
            .count()
    });
    if running > 0 {
        parts.push(format!("◐ {running} running"));
    }
    if app
        .state
        .ask
        .as_ref()
        .is_some_and(|ask| ask.params.session_id == session.0)
    {
        parts.push("⚠ waiting on you".to_owned());
    }
    (!parts.is_empty()).then(|| parts.join("  ·  "))
}

fn strip_line(app: &App, theme: &Theme) -> Option<Line<'static>> {
    let working = app
        .state
        .focused_session()
        .and_then(|id| app.state.sessions.get(&id))
        .is_some_and(|row| row.status == SessionStatus::Working);
    let mut spans = Vec::new();
    if let Some(note) = &app.state.status_note {
        spans.push(Span::styled(
            format!(" {note}"),
            Style::default().fg(theme.warning),
        ));
    }
    if let Some(text) = strip_text(app) {
        let lead = if spans.is_empty() { " " } else { "  ·  " };
        spans.push(Span::styled(format!("{lead}{text}"), theme.muted_style()));
    }
    if working {
        let mut line = working_line(None, app.spinner, false, theme);
        line.spans.extend(spans);
        return Some(line);
    }
    (!spans.is_empty()).then(|| Line::from(spans))
}

fn render_strip(app: &App, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let Some(line) = strip_line(app, theme) else {
        return;
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn render_ask(app: &App, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let Some(ask) = &app.state.ask else { return };
    let block = ratatui::widgets::Block::bordered()
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(Style::default().fg(theme.warning))
        .title(" permission ");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let area = inner;
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
    let block = ratatui::widgets::Block::bordered()
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(theme.accent_style())
        .title(" palette ")
        .style(Style::default().bg(theme.selection_bg()));
    let mut lines = vec![Line::from(vec![
        Span::styled("find ", theme.dim_style()),
        Span::styled(query.clone(), theme.accent_style()),
        Span::styled("▌", theme.accent_style()),
    ])];
    let matches = app.navigator_matches(query);
    let visible = usize::from(height.saturating_sub(4));
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
                label_of(&app.state.sessions, id, 30),
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
    lines.push(Line::styled(
        keys::hint(false, app.cmd_hints),
        theme.dim_style(),
    ));
    frame.render_widget(Paragraph::new(lines).block(block), popup);
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
    let focused = app.state.focused_session();
    let row = focused.as_ref().and_then(|id| app.state.sessions.get(id));
    let info = focused
        .as_ref()
        .and_then(|id| app.state.status.get(id))
        .cloned()
        .unwrap_or_default();
    let subagents = focused
        .as_ref()
        .and_then(|id| app.state.children.get(id))
        .map_or(0, |rows| {
            rows.iter()
                .filter(|child| child.status == yi_types::subagent::ChildStatus::Running)
                .count()
        });
    let input = StatusInput {
        model: info.model,
        thinking: info.effort,
        mode: None,
        cwd: row.map_or_else(|| app.state.root.clone(), |row| row.root.clone()),
        branch: None,
        cost: info.cost,
        session_name: row.map(SessionRow::label).unwrap_or_default(),
        subagents,
        context_used: info.context_used,
        context_window: info.context_window,
        focused_child: None,
    };
    let mut line = yi_tui::status::render(&input, usize::from(area.width), theme);
    let link = match &app.state.link {
        Link::Connecting => Some(Span::styled(" connecting… ", theme.dim_style())),
        Link::Connected => None,
        Link::Disconnected { reason } => {
            let short: String = reason.chars().take(28).collect();
            Some(Span::styled(
                format!(" ✕ disconnected: {short} "),
                Style::default().fg(theme.error),
            ))
        }
    };
    if let Some(link) = link {
        line.spans.insert(0, link);
    }
    if app.state.dropped_frames > 0 {
        line.spans.push(Span::styled(
            format!("  dropped:{}", app.state.dropped_frames),
            theme.dim_style(),
        ));
    }
    frame.render_widget(Paragraph::new(line), area);
}
