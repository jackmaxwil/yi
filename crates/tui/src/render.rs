use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use crate::app::{App, Bottom, ORB_COLS, ORB_ROWS};
use crate::cell::{Cell, TaskStatus, TranscriptMode};
use crate::hud::GoalView;
use crate::motion::elapsed_ms;
use crate::popup::BottomView;
use crate::port::SessionPort;
use crate::status::{StatusInput, working_line};
use crate::term;

const LIVE_TAIL_MIN: usize = 6;

pub fn live_tail_rows(rows: usize) -> usize {
    (rows / 2).max(LIVE_TAIL_MIN)
}

pub fn keep_last(lines: Vec<Line<'static>>, rows: usize) -> Vec<Line<'static>> {
    let skip = lines.len().saturating_sub(rows);
    lines.into_iter().skip(skip).collect()
}

pub fn live_tail(lines: Vec<Line<'static>>, rows: usize) -> Vec<Line<'static>> {
    keep_last(lines, live_tail_rows(rows))
}

fn tree_rows(rows: usize) -> usize {
    (rows / 2).max(5).min(rows.saturating_sub(9)).max(1)
}

pub fn draw<B>(
    app: &mut App,
    terminal: &mut crate::terminal::Terminal<B>,
    port: Option<&dyn SessionPort>,
) where
    B: ratatui::backend::Backend + std::io::Write,
{
    term::sync_frame(terminal, |terminal| draw_frame(app, terminal, port));
}

fn draw_frame<B>(
    app: &mut App,
    terminal: &mut crate::terminal::Terminal<B>,
    port: Option<&dyn SessionPort>,
) where
    B: ratatui::backend::Backend + std::io::Write,
{
    if let Some(title) = app.take_title() {
        let osc = crate::term::window_title_osc(&title);
        let _ = terminal.backend_mut().write_all(osc.as_bytes());
    }
    if app.take_pending_clear() {
        clear_screen(terminal);
    }
    let commits = std::mem::take(&mut app.pending_commit);
    // OSC 133 semantic prompt zones: Ghostty and iTerm2 use these to jump
    // between prompts, so a user turn is navigable in the terminal's own UI.
    let mark = std::mem::take(&mut app.pending_prompt_mark);
    let mode_changed = app.take_pending_repaint();
    schedule_reflow(app);
    if mark {
        let _ = terminal.backend_mut().write_all(b"\x1b]133;A\x07");
    }
    let _ = term::commit_lines(terminal, commits);
    if mark {
        let _ = terminal
            .backend_mut()
            .write_all(b"\x1b]133;B\x07\x1b]133;C\x07");
    }
    if mode_changed {
        app.reflow.schedule_immediate(std::time::Instant::now());
    }
    let reflow_theme = app.theme;
    run_reflow(app, terminal, app.content_width(), &reflow_theme);
    let goal = port.and_then(|port| port.goal());
    let memory = port
        .and_then(|port| port.memory())
        .and_then(|feed| feed.hud());
    app.sync_port(port);
    let total = u16::try_from(app.rows).unwrap_or(u16::MAX);
    let layout = layout_chat(app, goal, memory, total);
    let resized = terminal
        .resize_viewport(layout.rows(), layout.floor)
        .unwrap_or(false);
    if resized || mode_changed {
        terminal.invalidate_viewport();
    }
    let mut placed = Placed::default();
    let _ = terminal.draw(|frame| {
        let area = frame.area();
        placed = paint_chat(app, &layout, frame.buffer_mut(), area);
    });
    app.orb_placement = placed.orb;
    app.logo_rows = logo_rows(placed.bottom);
}

fn live_lines(app: &App, spinner: usize, theme: &crate::colors::Theme) -> Vec<Line<'static>> {
    let content_width = app.content_width();
    let mut live_lines: Vec<Line<'static>> = Vec::new();
    // Invariant: a held run of reads ended before any live thought began, so it renders first.
    if !app.explored.is_empty() {
        live_lines.extend(Cell::Explored(app.explored.clone()).lines(
            content_width,
            theme,
            app.mode,
            spinner,
        ));
    }
    if app.live_thought.len() > app.live_thought_cut {
        // Reasoning-heavy models stream thought long before prose, so show its dim tail and
        // the screen is never silently blank mid-turn; prose does not displace it.
        let tail = app
            .live_thought
            .get(app.live_thought_cut..app.pacing.thought.shown())
            .unwrap_or_default();
        let tail = crate::transcript::fence_tail(tail, live_tail_rows(app.rows));
        let tail = crate::transcript::close_spans(&tail);
        live_lines.extend(live_tail(app.thought_block(&tail), app.rows));
    }
    if !app.live_markdown.is_empty() {
        let tail = app
            .live_markdown
            .get(app.live_cut..app.pacing.prose.shown())
            .unwrap_or_default();
        let tail = match app.live_reopen {
            Some(_) => std::borrow::Cow::Borrowed(tail),
            None => crate::transcript::close_spans(tail),
        };
        let _span = yi_types::trace::span("tui.live_tail");
        live_lines.extend(app.prose_block(&tail).0);
    }
    if let Some(pen) = &app.pen {
        live_lines.extend(Cell::Tool(pen.card()).lines(content_width, theme, app.mode, spinner));
    }
    for tool in &app.live_tools {
        live_lines.extend(tool.lines(content_width, theme, app.mode, spinner));
    }
    for id in &app.task_order {
        if let Some(state) = app.tasks.get(id)
            && !app.committed_tasks.contains(id)
        {
            let mut cell = state.cell.clone();
            if state.finished.is_none() {
                cell.elapsed_ms = elapsed_ms(state.started);
            }
            live_lines.extend(cell.lines(content_width, theme, app.mode, spinner));
        }
    }
    live_lines
}

pub struct ChatLayout {
    live: Vec<Line<'static>>,
    working: Vec<Line<'static>>,
    hud: Vec<Line<'static>>,
    bottom: Option<Vec<Line<'static>>>,
    status: Line<'static>,
    composer_height: u16,
    show_working: bool,
    kitty: bool,
    pub floor: u16,
}

impl ChatLayout {
    pub fn rows(&self) -> u16 {
        u16::try_from(self.live.len())
            .unwrap_or(u16::MAX)
            .saturating_add(self.floor)
    }
}

/// Everything the chat shows for a frame of `total` rows, assembled from the app alone;
/// the live tail is trimmed to what the rest leaves.
pub fn layout_chat(
    app: &mut App,
    goal: Option<GoalView>,
    memory: Option<String>,
    total: u16,
) -> ChatLayout {
    let spinner = app.spinner_phase();
    let theme = app.theme;
    let width = app.width;

    let mut live_lines = live_lines(app, spinner, &theme);
    let hud_lines = if app.hud_hidden {
        Vec::new()
    } else {
        crate::hud::render(&crate::hud::input(app, goal, memory), &theme)
    };

    let selected = &app.selection.model;
    // OpenRouter is the one router yi reaches; its `vendor/` prefix names the maker, not the route.
    let routed = selected.provider == "openrouter";
    let model = match selected.id.split_once('/') {
        Some((_, name)) if routed => name.to_owned(),
        _ => selected.id.clone(),
    };
    let provider = routed.then(|| selected.provider.clone());
    // As the footer: a read is expected from the second request, on a route that prices one.
    let expected = app.requests > 1
        && selected
            .cost
            .cache_read
            .as_f64()
            .is_some_and(|price| price > 0.0);
    let status_input = StatusInput {
        model,
        provider,
        thinking: (app.selection.effort != yi_types::model::Effort::Off)
            .then(|| app.selection.effort.to_string()),
        mode: (app.mode != TranscriptMode::default()).then(|| app.mode.label().to_owned()),
        cwd: app.options.cwd.clone(),
        lane: app.options.lane.clone(),
        branch: app.branch.clone(),
        landing: app.landing.as_ref().and_then(|landing| {
            crate::status::landing_segment(landing, app.landing_at.map(|at| at.elapsed()))
        }),
        cost: app.spent.label(),
        cache: app.session_tokens.cache_label(expected),
        session_name: if app.status_name_hidden {
            String::new()
        } else {
            app.options.session_name.clone()
        },
        subagents: app
            .tasks
            .values()
            .filter(|s| s.cell.status == TaskStatus::Running)
            .count(),
        context_used: app.context_used,
        context_window: app.options.context_window,
        focused_child: app.focused.clone(),
    };
    let status_row = crate::status::render(&status_input, width, &theme);
    let bottom_lines: Option<Vec<Line<'static>>> = if let Some(tree) = &app.tree {
        Some(tree.lines(width, &theme, tree_rows(app.rows)))
    } else if let Some(plan_tree) = &app.plan_tree {
        Some(plan_tree.lines(width, &theme, tree_rows(app.rows)))
    } else {
        match &app.bottom {
            Some(Bottom::Approval(view, _)) => Some(view.lines(width, &theme)),
            Some(Bottom::Command(popup) | Bottom::File(popup)) => Some(popup.lines(width, &theme)),
            Some(Bottom::Agents(popup)) => Some(popup.lines(width, &theme)),
            Some(Bottom::Model(popup)) => Some(popup.lines(width, &theme)),
            None => None,
        }
    };
    let show_working = app.running || matches!(app.bottom, Some(Bottom::Approval(..)));
    let orb_state = app.orb_state();
    // On kitty the mark is always on screen, spelling `Yi` at rest and rearranging into
    // the orb for the turn. Elsewhere the plain spinner line appears only while a turn runs.
    let esc_armed = app
        .esc_armed_at
        .is_some_and(|at| at.elapsed() < crate::input::ESC_WINDOW);
    let working: Vec<Line<'static>> = if app.kitty {
        let mut rows: Vec<Line<'static>> = (0..ORB_ROWS).map(|_| Line::default()).collect();
        if let (Some(mid), Some(label)) = (rows.get_mut(1), app.working_label()) {
            let hint = if esc_armed {
                "esc again to interrupt"
            } else {
                "[esc] interrupt"
            };
            *mid = Line::from(vec![
                Span::raw(" ".repeat(usize::from(ORB_COLS) + 2)),
                Span::styled(label, ratatui::style::Style::default().fg(theme.text)),
                Span::styled(format!("  {hint}"), theme.dim_style()),
            ]);
        }
        rows
    } else if orb_state.is_some() {
        vec![working_line(
            app.working_label().as_deref(),
            spinner,
            esc_armed,
            &theme,
        )]
    } else {
        Vec::new()
    };
    let border = if app.running {
        theme.dim_style()
    } else {
        theme.muted_style()
    };
    app.bind_reply();
    app.composer
        .set_frame(border, theme.dim_style(), app.reply_title());
    let composer_height = app.composer.desired_height();

    let bottom_height = bottom_lines.as_ref().map_or(composer_height, |lines| {
        u16::try_from(lines.len()).unwrap_or(composer_height)
    });
    let rows = |lines: &[Line<'static>]| u16::try_from(lines.len()).unwrap_or(u16::MAX);
    // Incident: `resize_viewport` clamps to the screen and `put` drops what no longer fits,
    // so an unbudgeted live tail cost an 8-row screen its status line to a floor of 6.

    // ponytail: one elastic member. If the floor alone outgrows the screen a
    // tall approval still clips; budget the HUD before building a priority order.
    let floor = rows(&hud_lines)
        .saturating_add(if show_working || app.kitty {
            rows(&working)
        } else {
            0
        })
        .saturating_add(bottom_height)
        .saturating_add(1);
    let budget = total.saturating_sub(floor);
    if rows(&live_lines) > budget {
        live_lines = keep_last(live_lines, usize::from(budget));
    }
    ChatLayout {
        live: live_lines,
        working,
        hud: hud_lines,
        bottom: bottom_lines,
        status: status_row,
        composer_height,
        show_working,
        kitty: app.kitty,
        floor,
    }
}

/// Where the last paint put the orb and the bottom block, in cells.
#[derive(Default)]
pub struct Placed {
    pub orb: Option<(u16, u16)>,
    pub bottom: Option<(u16, u16)>,
}

/// The picker's first model row is one below its query line; the slot is its second cell.
fn logo_rows(bottom_at: Option<(u16, u16)>) -> Option<(u16, u16)> {
    bottom_at.map(|(col, row)| (col.saturating_add(1), row.saturating_add(1)))
}

/// Paints a layout into any rectangle of a buffer; returns where the orb and the bottom
/// block belong.
pub fn paint_chat(app: &App, layout: &ChatLayout, buffer: &mut Buffer, area: Rect) -> Placed {
    let mut orb_at = None;
    let mut bottom_at = None;
    let mut y = area.top();
    let put = |buffer: &mut Buffer, lines: &[Line<'static>], y: &mut u16| {
        let height = u16::try_from(lines.len()).unwrap_or(0);
        if height == 0 || *y >= area.bottom() {
            return;
        }
        let height = height.min(area.bottom().saturating_sub(*y));
        let rect = Rect::new(area.left(), *y, area.width, height);
        Widget::render(Paragraph::new(lines.to_vec()), rect, buffer);
        *y = y.saturating_add(height);
    };
    put(buffer, &layout.live, &mut y);
    // Incident (D46): the mark trails the live tail. §17.3 makes the viewport's top row the
    // commit boundary, so a leading mark walked down a paragraph at a time.
    if layout.show_working || layout.kitty {
        if layout.kitty && y < area.bottom() {
            orb_at = Some((area.left(), y));
        }
        put(buffer, &layout.working, &mut y);
    }
    put(buffer, &layout.hud, &mut y);
    match &layout.bottom {
        Some(lines) => {
            bottom_at = Some((area.left(), y));
            put(buffer, lines, &mut y);
        }
        None => {
            if y < area.bottom() {
                let height = layout.composer_height.min(area.bottom().saturating_sub(y));
                let rect = Rect::new(
                    area.left().saturating_add(1),
                    y,
                    area.width.saturating_sub(2),
                    height,
                );
                Widget::render(&app.composer.textarea, rect, buffer);
                y = y.saturating_add(height);
            }
        }
    }
    put(buffer, std::slice::from_ref(&layout.status), &mut y);
    Placed {
        orb: orb_at,
        bottom: bottom_at,
    }
}

/// Transcript and live frame scroll as one column; `scroll` zero follows the turn, above holds.
pub fn paint_pane(
    app: &mut App,
    goal: Option<GoalView>,
    buffer: &mut Buffer,
    area: Rect,
    scroll: &mut usize,
) -> Option<(usize, usize)> {
    let _span = yi_types::trace::span("tui.paint_pane");
    // A pane never scrolls anything off: `History` keeps every cell, so the
    // terminal-bound commits and the clear are drained here and dropped.
    let _ = app.take_commits();
    if app.take_pending_clear() {
        *scroll = 0;
    }
    let _ = app.take_title();
    let _ = app.take_pending_repaint();
    app.set_width(usize::from(area.width));
    app.set_rows(usize::from(area.height));
    let mut layout = layout_chat(app, goal, None, u16::MAX);
    let live = std::mem::take(&mut layout.live);
    let floor = layout.floor.min(area.height);
    let above = usize::from(area.height.saturating_sub(floor));
    let (width, theme, mode) = (app.content_width(), app.theme, app.mode);
    let (shown, owners, thumb) = if *scroll == 0 {
        app.pane_hold = None;
        let history = yi_types::trace::span("tui.history_rows");
        let want = above.saturating_sub(live.len()).max(1);
        let (mut column, mut owners) = app.history.lines(width, &theme, mode, want);
        drop(history);
        column.extend(live);
        let start = column.len().saturating_sub(above);
        (
            column.split_off(start),
            owners.split_off(start.min(owners.len())),
            None,
        )
    } else {
        // Incident: counting committed rows drifted a held view; now rows below a pinned cell.
        let hold = app
            .pane_hold
            .filter(|&(w, m, _, _)| (w, m) == (width, mode));
        if let Some((_, _, pinned, extent)) = hold {
            let (at, now, _) = app.history.tail(width, &theme, mode, 0, pinned);
            if at == pinned {
                let grown = now.len().saturating_add(live.len()).saturating_sub(above);
                *scroll = scroll.saturating_add(grown).saturating_sub(extent);
            }
        }
        let need = above.saturating_add(*scroll).saturating_sub(live.len());
        let at_most = hold.map_or(usize::MAX, |(_, _, pinned, _)| pinned);
        let (from, mut column, owners) = app.history.tail(width, &theme, mode, need, at_most);
        let history_rows = column.len();
        column.extend(live);
        *scroll = (*scroll).min(column.len().saturating_sub(above));
        app.pane_hold = Some((width, mode, from, column.len().saturating_sub(above)));
        let end = column.len().saturating_sub(*scroll);
        let start = end.saturating_sub(above);
        let shown = column.get(start..end).unwrap_or_default().to_vec();
        let owners = owners
            .get(start..end.min(history_rows))
            .unwrap_or_default()
            .to_vec();
        let cells = app.history.len().saturating_sub(from).max(1);
        let before = from.saturating_mul(history_rows) / cells;
        let total = before
            .saturating_add(column.len())
            .saturating_add(usize::from(floor));
        let thumb =
            (total > usize::from(area.height)).then_some((total, before.saturating_add(start)));
        (shown, owners, thumb)
    };
    let mut owned: Vec<Option<usize>> = owners.into_iter().map(Some).collect();
    owned.resize(shown.len(), None);
    app.pane_rows = (area.top(), owned);
    let rows = u16::try_from(shown.len()).unwrap_or(0);
    if rows > 0 {
        let rect = Rect::new(area.left(), area.top(), area.width, rows);
        Widget::render(Paragraph::new(shown), rect, buffer);
    }
    let chat_area = Rect::new(
        area.left(),
        area.top().saturating_add(rows),
        area.width,
        area.height.saturating_sub(rows),
    );
    let placed = paint_chat(app, &layout, buffer, chat_area);
    app.orb_placement = placed.orb;
    app.logo_rows = logo_rows(placed.bottom);
    thumb
}

/// The transcript above the viewport belongs to a branch that no longer exists and sits in
/// scrollback no repaint reaches, so the scrollback goes and the viewport re-anchors.
fn clear_screen<B>(terminal: &mut crate::terminal::Terminal<B>)
where
    B: ratatui::backend::Backend + std::io::Write,
{
    // The screen itself is cleared through the backend so the headless screen
    // clears too; only the scrollback erase (`ESC[3J`) has no backend call.
    let _ = ratatui::backend::Backend::clear(terminal.backend_mut());
    let _ = terminal.backend_mut().write_all(b"\x1b[3J");
    let _ = ratatui::backend::Backend::set_cursor_position(
        terminal.backend_mut(),
        ratatui::layout::Position::ORIGIN,
    );
    let _ = std::io::Write::flush(terminal.backend_mut());
    let area = terminal.viewport_area();
    terminal.set_viewport_area(Rect {
        x: 0,
        y: 0,
        width: area.width,
        height: area.height,
    });
    terminal.invalidate_viewport();
}

/// A width change invalidates every wrapped row in scrollback. Each event pushes the
/// deadline out, so a drag rebuilds once at the settled width.
fn schedule_reflow(app: &mut App) {
    let width = u16::try_from(app.width).unwrap_or(u16::MAX);
    let change = app.reflow.note_width(width);
    if change.initialized {
        return;
    }
    if !change.changed && !app.reflow.reflow_needed_for_width(width) {
        return;
    }
    if app.running {
        // A rebuild now can only render the partial that exists now; the end of
        // the stream has to repeat it from the settled text.
        app.reflow.mark_resize_requested_during_stream();
    }
    app.reflow
        .schedule_debounced(Some(width), std::time::Instant::now());
}

/// §17.3: clear scrollback and the visible screen, then re-emit the retained transcript at the
/// current width. Row-capped at render, so rows the terminal would not retain are not written.
fn run_reflow<B>(
    app: &mut App,
    terminal: &mut crate::terminal::Terminal<B>,
    width: usize,
    theme: &crate::colors::Theme,
) where
    B: ratatui::backend::Backend + std::io::Write,
{
    let now = std::time::Instant::now();
    if !app.reflow.pending_is_due(now) {
        return;
    }
    app.reflow.clear_pending_reflow();
    let target = u16::try_from(app.width).unwrap_or(u16::MAX);
    app.reflow.mark_reflowed_width(target);
    if app.running {
        app.reflow.mark_ran_during_stream();
    }
    if app.history.is_empty() {
        return;
    }
    let lines = app
        .history
        .replay(width, theme, app.mode(), crate::reflow::reflow_max_rows());
    if terminal.clear_scrollback_and_visible_screen().is_err() {
        return;
    }
    // The mark went with the screen; the next tick has to place it again.
    app.mark_orb_stale();
    let _ = crate::term::commit_lines(terminal, lines);
    terminal.invalidate_viewport();
}
