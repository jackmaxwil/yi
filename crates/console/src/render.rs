//! compute_view(&mut) then render(&): geometry and cache mutation first,
//! drawing reads only; ViewState doubles as the mouse hit-test table.

use std::collections::HashMap;

use crate::model::{SessionId, SessionRow, SidebarMode, now_ms};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use yi_tui::SessionPort;
use yi_tui::colors::{Theme, name_tile, tile_style};
use yi_tui::diffview::{self, DiffBudget};

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
    pub chat: bool,
}

pub struct Hits {
    pub sidebar_width: u16,
    pub sidebar_rows: Vec<(u16, usize)>,
    /// (column, row, session) of every avatar cell on screen; the kitty pass places there.
    pub avatars: Vec<(u16, u16, SessionId)>,
    pub root_rows: Vec<(u16, usize)>,
    pub panes: Vec<(PaneId, Rect)>,
    pub splits: Vec<SplitBorder>,
    pub tabs: Vec<(Rect, usize)>,
}

pub struct ViewState {
    pub sidebar: Rect,
    pub roots: Rect,
    pub editor_cursor: Option<(u16, u16)>,
    pub tab_bar: Option<Rect>,
    pub panes_area: Rect,
    pub panes: Vec<PaneView>,
    pub split_borders: Vec<SplitBorder>,
    pub banner: Option<Rect>,
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
    let banner_rows = u16::from(
        app.state.banner.is_some()
            || app.state.link != Link::Connected
            || app.state.dropped_frames > 0,
    );
    let (transcript_area, banner) = split_off_bottom(main, banner_rows);
    let banner = (banner_rows > 0).then_some(banner);
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
        let chat =
            app.state.panes.get(&pane_rect.id).is_some_and(|pane| {
                matches!(pane.content, PaneContent::Session { chat: Some(_), .. })
            });
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
            chat,
        });
    }

    let sidebar_rows: Vec<(u16, usize)> = sidebar_lines(app, theme, sidebar.height)
        .iter()
        .enumerate()
        .filter_map(|(offset, (index, _))| {
            let y = sidebar.y.checked_add(u16::try_from(offset).ok()?)?;
            index.map(|index| (y, index))
        })
        .collect();
    let avatars = sidebar_rows
        .iter()
        .filter_map(|(y, index)| {
            let id = app.state.order.get(*index)?.clone();
            Some((sidebar.x.saturating_add(2), *y, id))
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
        avatars,
        root_rows,
        panes: panes.iter().map(|pane| (pane.id, pane.rect)).collect(),
        splits: split_borders.clone(),
        tabs: tab_hits,
    });

    ViewState {
        sidebar,
        roots: roots_area,
        editor_cursor,
        tab_bar,
        panes_area,
        panes,
        split_borders,
        banner,
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
        PaneContent::Session { session, .. } => {
            let title = session.as_ref().map_or_else(
                || "❯ no session".to_owned(),
                |id| format!("❯ {}", label_of(sessions, id, 14)),
            );
            (title, Vec::new(), None)
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

pub fn render(app: &mut App, frame: &mut Frame<'_>, view: &mut ViewState, theme: &Theme) {
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
    let framed = view.framed;
    let kitty = app.kitty;
    for pane in &mut view.panes {
        let inner = pane.rect.inner(pane_margin(framed));
        let dim = crowd && !pane.focused;
        if pane.chat {
            paint_chat_pane(app, pane, inner, frame.buffer_mut(), kitty, dim);
            continue;
        }
        let body = Paragraph::new(pane.lines.clone());
        let body = if dim {
            body.style(Style::default().add_modifier(Modifier::DIM))
        } else {
            body
        };
        frame.render_widget(body, inner);
    }
    if view.framed {
        render_borders(app, frame, view, theme);
    }
    render_thumbs(frame, view, theme);
    let notebook = app
        .state
        .focused_pane_id()
        .and_then(|id| app.state.panes.get(&id))
        .and_then(|pane| match &pane.content {
            PaneContent::Notebook { input, .. } => Some(input.as_ref()),
            _ => None,
        });
    if let (Some(input), Some(pane)) = (notebook, view.panes.iter().find(|pane| pane.focused)) {
        let inner = pane.rect.inner(pane_margin(framed));
        let (_, row) = split_off_bottom(inner, 3);
        frame.render_widget(input, row);
    }
    if let Some(area) = view.banner {
        render_banner(app, frame, area, theme);
    }
    if matches!(app.state.mode, Mode::Navigator { .. }) {
        render_navigator(app, frame, view, theme);
    }
}

/// Solo's chat into the pane's rectangle; the focused pane alone carries the orb.
fn paint_chat_pane(
    app: &mut App,
    view: &mut PaneView,
    inner: Rect,
    buffer: &mut Buffer,
    kitty: bool,
    dim: bool,
) {
    let Some(pane) = app.state.panes.get_mut(&view.id) else {
        return;
    };
    let PaneContent::Session {
        chat: Some(chat), ..
    } = &mut pane.content
    else {
        return;
    };
    chat.app.set_kitty(kitty && view.focused);
    let goal = chat.port.goal();
    let mut scroll = pane.scroll_from_bottom;
    view.scroll = yi_tui::render::paint_pane(&mut chat.app, goal, buffer, inner, &mut scroll);
    pane.scroll_from_bottom = scroll;
    if dim {
        for y in inner.top()..inner.bottom() {
            for x in inner.left()..inner.right() {
                if let Some(cell) = buffer.cell_mut((x, y)) {
                    let style = cell.style().add_modifier(Modifier::DIM);
                    cell.set_style(style);
                }
            }
        }
    }
}

fn render_banner(app: &App, frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let mut spans = Vec::new();
    match &app.state.link {
        Link::Connecting => spans.push(Span::styled(" connecting… ", theme.dim_style())),
        Link::Connected => {}
        Link::Disconnected { reason } => {
            let short: String = reason.chars().take(28).collect();
            spans.push(Span::styled(
                format!(" ✕ disconnected: {short} "),
                Style::default().fg(theme.error),
            ));
        }
    }
    if let Some(note) = &app.state.banner {
        spans.push(Span::styled(
            format!(" {note}"),
            Style::default().fg(theme.warning),
        ));
    }
    if app.state.dropped_frames > 0 {
        spans.push(Span::styled(
            format!("  dropped:{}", app.state.dropped_frames),
            theme.dim_style(),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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
        let label = app
            .state
            .panes
            .get(&pane.id)
            .and_then(|p| p.session())
            .and_then(|s| app.state.sessions.get(s))
            .map(SessionRow::label);
        let mut x = r.x.saturating_add(2);
        let mut max = r.width.saturating_sub(4);
        if let Some(label) = &label {
            let tile = Span::styled(format!(" {}", name_tile(label)), tile_style(label));
            buffer.set_span(x, r.y, &tile, max);
            x = x.saturating_add(3);
            max = max.saturating_sub(3);
        }
        let text = format!(" {} {} ", status.glyph(), pane.title);
        let text: String = text.chars().take(usize::from(max)).collect();
        let style = if pane.focused {
            theme
                .accent_style()
                .add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            theme.dim_style()
        };
        let span = Span::styled(text, style);
        buffer.set_span(x, r.y, &span, max);
    }
}

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

pub fn sidebar_lines(app: &App, theme: &Theme, height: u16) -> Vec<(Option<usize>, Line<'static>)> {
    let mut rows = Vec::new();
    let rail = app.state.sidebar == SidebarMode::Rail;
    let multi_root = !rail && app.state.roots().len() > 1;
    let mut current_root: Option<&str> = None;
    let focused = app.state.focused_session();
    let now = now_ms();
    let mut slot = 0_usize;
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
        let is_cursor = index == app.state.selected && app.state.zone == Zone::Sidebar;
        slot = slot.saturating_add(1);
        let label = row.label();
        let row_bg = if is_cursor {
            Some(theme.selection_bg())
        } else if is_focused {
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
        let mut spans = vec![
            Span::styled(slot_text, on_row(number)),
            Span::styled(name_tile(&label), tile_style(&label)),
            Span::styled(" ".to_owned(), on_row(Style::default())),
            Span::styled(
                row.status.glyph().to_owned(),
                on_row(status_style(theme, row.status)),
            ),
        ];
        if rail {
            rows.push((Some(index), Line::from(spans)));
            continue;
        }
        let name: String = label.chars().take(NAME_WIDTH).collect();
        let pad = NAME_WIDTH.saturating_sub(name.chars().count());
        let name_style = if is_focused {
            theme.accent_style().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        let tail = if row.attached { "·" } else { " " };
        spans.push(Span::styled(" ".to_owned(), on_row(Style::default())));
        spans.push(Span::styled(name, on_row(name_style)));
        spans.push(Span::styled(
            format!("{}{:>4}{tail}", " ".repeat(pad), age(now, row.recency())),
            on_row(theme.dim_style()),
        ));
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
    if app.state.sidebar == SidebarMode::Rail {
        let armed = app.state.zone == Zone::Sidebar || matches!(app.state.mode, Mode::Prefix);
        let style = if armed {
            theme.accent_style()
        } else {
            theme.dim_style()
        };
        let x = area.x.saturating_add(area.width).saturating_sub(1);
        let buffer = frame.buffer_mut();
        for y in area.top()..area.bottom() {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_symbol("│");
                cell.set_style(style);
            }
        }
    }
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
