use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use yi_runtime::AgentSession;

use crate::app::{App, Bottom, ORB_COLS, ORB_ROWS, elapsed_ms};
use crate::cell::{Cell, TaskStatus, TranscriptMode};
use crate::hud::GoalView;
use crate::popup::BottomView;
use crate::status::{StatusInput, working_line};
use crate::term;

const LIVE_TAIL_MIN: usize = 6;

/// A markdown table holds no blank line, so nothing commits until the message
/// ends and the whole table sits here — a fixed six-row tail cut its head off
/// mid-stream. Half the screen keeps the viewport off [`crate::terminal::Terminal::insert_before`]'s path.
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

/// Swaps the static `∴` on a live thought's header for the breathing starburst.
/// Every frame is one cell wide, so the row cannot reflow under it.
fn pulse_thought_header(lines: &mut [Line<'static>], spinner_phase: usize) {
    let glyph = crate::motion::thinking_glyph(crate::motion::elapsed_of(spinner_phase));
    for line in lines.iter_mut() {
        for span in &mut line.spans {
            if span.content.contains('∴') {
                span.content = span.content.replace('∴', &glyph.to_string()).into();
                return;
            }
        }
    }
}

/// OMP's sizing: half the terminal, floor 5, less the panel's chrome.
fn tree_rows(rows: usize) -> usize {
    (rows / 2).max(5).min(rows.saturating_sub(9)).max(1)
}

pub fn draw<B>(
    app: &mut App,
    terminal: &mut crate::terminal::Terminal<B>,
    session: Option<&AgentSession>,
) where
    B: ratatui::backend::Backend + std::io::Write,
{
    term::sync_frame(terminal, |terminal| draw_frame(app, terminal, session));
}

fn draw_frame<B>(
    app: &mut App,
    terminal: &mut crate::terminal::Terminal<B>,
    session: Option<&AgentSession>,
) where
    B: ratatui::backend::Backend + std::io::Write,
{
    if let Some(title) = app.take_title() {
        let osc = format!("\x1b]2;{title}\x07");
        let _ = terminal.backend_mut().write_all(osc.as_bytes());
    }
    let commits = std::mem::take(&mut app.pending_commit);
    // OSC 133 semantic prompt zones (OMP `user-message.ts:25-28`): the shell
    // integration in Ghostty and iTerm2 uses these to jump between prompts, so
    // a user turn is navigable in the terminal's own UI.
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
    run_reflow(app, terminal, app.width.saturating_sub(2), &reflow_theme);
    if let Some(usage) = session.and_then(AgentSession::last_usage) {
        app.context_used = u64::try_from(usage.total_tokens).unwrap_or(0);
        app.cost_total = usage.cost.total.as_f64().unwrap_or(0.0);
    }
    let goal = session.and_then(AgentSession::store).and_then(|store| {
        yi_runtime::session_store::lock_session(&store)
            .goal()
            .map(|goal| GoalView {
                objective: goal.objective,
                status: goal.status.as_str().to_owned(),
                tokens_used: goal.tokens_used,
                token_budget: goal.token_budget,
            })
    });
    let spinner = app.spinner_phase();
    let theme = app.theme;
    let width = app.width;

    let content_width = width.saturating_sub(2);
    let mut live_lines: Vec<Line<'static>> = Vec::new();
    if !app.live_thought.is_empty() {
        // Reasoning-heavy models stream thought long before prose; show its
        // dim tail so the screen is never silently blank mid-turn. It holds
        // that place once prose starts, rather than being displaced by it.
        let tail = app
            .live_thought
            .get(app.live_thought_cut..)
            .unwrap_or_default();
        let mut rendered = crate::cell::thought_lines(
            tail,
            content_width,
            &theme,
            app.mode,
            app.live_thought_cut == 0,
        );
        // The label pulses only while it is still live and still here: past the
        // first committed slice the tail has no header and this is a no-op.
        pulse_thought_header(&mut rendered, spinner);
        live_lines.extend(live_tail(rendered, app.rows));
    }
    if !app.live_markdown.is_empty() {
        let tail = app.live_markdown.get(app.live_cut..).unwrap_or_default();
        let rendered = crate::cell::gutter(
            crate::markdown::render(
                tail,
                content_width.saturating_sub(crate::cell::GUTTER.len()),
                &theme,
            ),
            app.live_cut == 0,
            &theme,
        );
        live_lines.extend(live_tail(rendered, app.rows));
    }
    // The run is held back from scrollback until it closes, so the live region
    // is the only place it can be seen while it is still growing.
    if !app.explored.is_empty() {
        live_lines.extend(Cell::Explored(app.explored.clone()).lines(
            content_width,
            &theme,
            app.mode,
            spinner,
        ));
    }
    for tool in &app.live_tools {
        live_lines.extend(tool.lines(content_width, &theme, app.mode, spinner));
    }
    for id in &app.task_order {
        if let Some(state) = app.tasks.get(id)
            && state.cell.status == TaskStatus::Running
        {
            let mut cell = state.cell.clone();
            cell.elapsed_ms = elapsed_ms(state.started);
            live_lines.extend(cell.lines(width, &theme, spinner));
        }
    }
    let hud_lines = if app.hud_hidden {
        Vec::new()
    } else {
        crate::hud::render(&app.hud_input(goal), &theme, spinner)
    };

    let status_input = StatusInput {
        model: app.selection.model.id.clone(),
        thinking: (app.selection.effort != yi_types::model::Effort::Off)
            .then(|| app.selection.effort.to_string()),
        mode: (app.mode != TranscriptMode::default()).then(|| app.mode.label().to_owned()),
        cwd: app.options.cwd.clone(),
        branch: None,
        cost: if app.cost_total > 0.0 {
            Some(format!("${:.2}", app.cost_total))
        } else {
            None
        },
        session_name: app.options.session_name.clone(),
        subagents: app
            .tasks
            .values()
            .filter(|s| s.cell.status == TaskStatus::Running)
            .count(),
        context_used: app.context_used,
        context_window: app.options.context_window,
        threshold_pct: Some(80),
        focused_child: app.focused.clone(),
    };
    let status_row = crate::status::render(&status_input, width, &theme);
    let bottom_lines: Option<Vec<Line<'static>>> = if let Some(tree) = &app.tree {
        Some(tree.lines(width, &theme, tree_rows(app.rows)))
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
    // U34: on a kitty terminal the mark is always on screen — at rest it spells
    // `Yi`, and it rearranges into the orb for the turn. Elsewhere the plain
    // spinner line appears only while a turn runs.
    let working: Vec<Line<'static>> = if app.kitty {
        let mut rows: Vec<Line<'static>> = (0..ORB_ROWS).map(|_| Line::default()).collect();
        if let (Some(mid), Some(state)) = (rows.get_mut(1), orb_state) {
            let label = app
                .intent
                .clone()
                .unwrap_or_else(|| state.label().to_owned());
            let hint = if app.esc_armed_at.is_some() {
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
            app.intent.as_deref(),
            spinner,
            app.esc_armed_at.is_some(),
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
    app.composer.set_frame(border, theme.dim_style());
    let composer_height = app.composer.desired_height();
    let orb_active = app.kitty;
    let orb_at: std::cell::Cell<Option<(u16, u16)>> = std::cell::Cell::new(None);
    let composer = &app.composer.textarea;

    let bottom_height = bottom_lines.as_ref().map_or(composer_height, |lines| {
        u16::try_from(lines.len()).unwrap_or(composer_height)
    });
    let rows = |lines: &[Line<'static>]| u16::try_from(lines.len()).unwrap_or(u16::MAX);
    // Incident: `resize_viewport` clamps to the screen and `put` silently drops
    // what no longer fits, so an unbudgeted live tail evicted the rows drawn
    // after it — an 8-row screen lost its status line to the hard floor of 6.

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
    let budget = u16::try_from(app.rows)
        .unwrap_or(u16::MAX)
        .saturating_sub(floor);
    if rows(&live_lines) > budget {
        live_lines = keep_last(live_lines, usize::from(budget));
    }
    let desired = rows(&live_lines).saturating_add(floor);
    // U36 (D47): the viewport resize touches nothing above itself. Painting one
    // window of freshly wrapped lines over content the emulator already reflowed
    // is what left the transcript showing fragments at two widths.
    let resized = terminal.resize_viewport(desired).unwrap_or(false);
    if resized || mode_changed {
        terminal.invalidate_viewport();
    }
    let _ = terminal.draw(|frame| {
        // The inline viewport's buffer area starts at area.y, not 0 — a rect
        // outside the area renders nowhere, silently.
        let area = frame.area();
        let mut y = area.top();
        let put = |frame: &mut crate::terminal::Frame, lines: &[Line<'static>], y: &mut u16| {
            let height = u16::try_from(lines.len()).unwrap_or(0);
            if height == 0 || *y >= area.bottom() {
                return;
            }
            let height = height.min(area.bottom() - *y);
            let rect = Rect::new(area.left(), *y, area.width, height);
            frame.render_widget(Paragraph::new(lines.to_vec()), rect);
            *y += height;
        };
        put(frame, &live_lines, &mut y);
        // Incident (D46): the mark trails the live tail. U13 makes the viewport's
        // top row the commit boundary, so a leading mark sat wedged mid-answer and
        // walked down one paragraph at a time, each move a kitty delete-and-replace.
        if show_working || app.kitty {
            if orb_active && y < area.bottom() {
                orb_at.set(Some((area.left(), y)));
            }
            put(frame, &working, &mut y);
        }
        put(frame, &hud_lines, &mut y);
        match &bottom_lines {
            Some(lines) => put(frame, lines, &mut y),
            None => {
                if y < area.bottom() {
                    let height = composer_height.min(area.bottom() - y);
                    let rect = Rect::new(area.left() + 1, y, area.width.saturating_sub(2), height);
                    frame.render_widget(composer, rect);
                    y += height;
                }
            }
        }
        put(frame, std::slice::from_ref(&status_row), &mut y);
    });
    app.orb_placement = orb_at.get();
    app.logo_target = if orb_state.is_some() { 1.0 } else { 0.0 };
}

/// U35/U36: a width change invalidates every wrapped row in scrollback. Each
/// event pushes the deadline out, so a drag rebuilds once at the settled width;
/// the first width observed initializes without scheduling.
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

/// U36: clear scrollback and the visible screen, then re-emit the retained
/// transcript at the current width. Row-capped while rendering from source, so
/// rows the terminal would not retain are never written at all.
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
