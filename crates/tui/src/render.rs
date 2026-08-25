use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use yi_runtime::AgentSession;

use crate::app::{App, Bottom, LIVE_TAIL_ROWS, ORB_COLS, ORB_ROWS, elapsed_ms};
use crate::cell::{Cell, TaskStatus, TranscriptMode};
use crate::hud::GoalView;
use crate::popup::BottomView;
use crate::status::{StatusInput, working_line};
use crate::term;

pub fn draw<B>(
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
    if mark {
        let _ = terminal.backend_mut().write_all(b"\x1b]133;A\x07");
    }
    let _ = term::commit_lines(terminal, commits);
    if mark {
        let _ = terminal
            .backend_mut()
            .write_all(b"\x1b]133;B\x07\x1b]133;C\x07");
    }
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
        let skip = rendered.len().saturating_sub(LIVE_TAIL_ROWS);
        live_lines.extend(rendered.into_iter().skip(skip));
    } else if !app.live_thought.is_empty() {
        // Reasoning-heavy models stream thought long before prose; show its
        // dim tail so the screen is never silently blank mid-turn.
        let cell = Cell::Thought {
            markdown: app.live_thought.clone(),
        };
        let rendered = cell.lines(content_width, &theme, TranscriptMode::Thinking, spinner);
        let skip = rendered.len().saturating_sub(LIVE_TAIL_ROWS);
        live_lines.extend(rendered.into_iter().skip(skip));
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
        model: app.options.model_label.clone(),
        thinking: None,
        mode: None,
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
        Some(tree.lines(width, &theme, 8))
    } else {
        match &app.bottom {
            Some(Bottom::Approval(view, _)) => Some(view.lines(width, &theme)),
            Some(Bottom::Command(popup) | Bottom::File(popup)) => Some(popup.lines(width, &theme)),
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
    let desired = rows(&live_lines)
        .saturating_add(rows(&hud_lines))
        .saturating_add(if show_working || app.kitty {
            rows(&working)
        } else {
            0
        })
        .saturating_add(bottom_height)
        .saturating_add(1);
    // codex resize reflow: when the viewport moves, the rows above it hold what
    // the emulator's re-wrap left behind, so rebuild them from the retained
    // transcript and force the viewport to repaint every cell.
    if terminal.resize_viewport(desired).unwrap_or(false) {
        let rows = usize::from(terminal.viewport_area().top());
        let history = app.history.lines(content_width, &theme, app.mode, rows);
        let _ = terminal.repaint_history(|buf| {
            let offset = rows.saturating_sub(history.len());
            for (i, line) in history.iter().enumerate() {
                let Ok(y) = u16::try_from(offset + i) else {
                    continue;
                };
                buf.set_line(0, y, line, buf.area.width);
            }
        });
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
        // U34: the mark leads the live region and the streaming answer flows
        // under it. Below the text it would slide down the screen with every
        // token; above, it holds one position, which is what an animation
        // needs and what makes it read as the agent rather than as a spinner.
        if show_working || app.kitty {
            if orb_active && y < area.bottom() {
                orb_at.set(Some((area.left(), y)));
            }
            put(frame, &working, &mut y);
        }
        put(frame, &live_lines, &mut y);
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
