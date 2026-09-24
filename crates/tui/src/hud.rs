use std::time::{Duration, Instant};

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use yi_runtime::todo::DEFAULT_PHASE;
use yi_types::plan::doc::TodoStateName;
use yi_types::todo::{TodoItem, TodoList};

use crate::colors::Theme;

#[derive(Debug, Clone, Default)]
pub struct HudInput {
    pub goal: Option<GoalView>,
    pub landing: Option<String>,
    pub plan: Option<PlanProgress>,
    pub todos: Option<TodoList>,
    pub todo_full: bool,
    pub steering: Vec<String>,
    pub follow_up: Vec<String>,
    pub memory: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanProgress {
    pub done: usize,
    pub total: usize,
    pub running: Option<String>,
}

impl PlanProgress {
    pub fn line(&self) -> String {
        let mut line = format!("Plan {}/{}", self.done, self.total);
        if let Some(running) = &self.running {
            line.push_str(&format!(" · now: {running}"));
        }
        line
    }
}

#[derive(Debug, Clone)]
pub struct GoalView {
    pub objective: String,
    pub status: String,
    pub tokens_used: u64,
    pub token_budget: Option<u64>,
}

pub const TODO_FULL_FOR: Duration = Duration::from_secs(10);
const TODO_COMPACT: usize = 4;

/// When the list's labels last changed: stepping an item keeps the clock, an append restarts it.
#[derive(Debug, Clone, Default)]
pub struct TodoClock {
    seen: Option<(Vec<String>, Instant)>,
}

impl TodoClock {
    pub fn observe(&mut self, list: Option<&TodoList>, now: Instant) {
        let Some(list) = list else {
            self.seen = None;
            return;
        };
        let labels: Vec<String> = list.items().map(|item| item.label.to_string()).collect();
        if self.seen.as_ref().is_none_or(|(seen, _)| *seen != labels) {
            self.seen = Some((labels, now));
        }
    }

    pub fn full(&self, now: Instant) -> bool {
        self.wake(now) != Duration::MAX
    }

    pub fn wake(&self, now: Instant) -> Duration {
        self.seen
            .as_ref()
            .and_then(|(_, at)| TODO_FULL_FOR.checked_sub(now.saturating_duration_since(*at)))
            .filter(|left| !left.is_zero())
            .unwrap_or(Duration::MAX)
    }
}

/// When folded, the step before the current one and two after, or three after if none is done.
pub fn todo_rows(
    list: Option<&TodoList>,
    full: bool,
    theme: &Theme,
) -> Option<(String, Vec<Line<'static>>)> {
    let list = list?;
    let progress = list.progress();
    if progress.open.saturating_add(progress.blocked) == 0 {
        return None;
    }
    let mut title = format!("Todos {}/{}", progress.done, progress.total);
    if progress.blocked > 0 {
        title.push_str(&format!(" · {} blocked", progress.blocked));
    }
    let current = list
        .items()
        .position(|item| item.state == TodoStateName::Running)
        .or_else(|| {
            list.items().position(|item| {
                matches!(item.state, TodoStateName::Pending | TodoStateName::Blocked)
            })
        })
        .unwrap_or(0);
    let start = if progress.done == 0 {
        current
    } else {
        current.saturating_sub(1)
    };
    let shown = if full {
        0..usize::MAX
    } else {
        start..start.saturating_add(TODO_COMPACT)
    };
    let mut rows = Vec::new();
    let mut number = 0usize;
    for phase in &list.phases {
        if full && (list.phases.len() > 1 || phase.name.as_str() != DEFAULT_PHASE) {
            rows.push(Line::from(Span::styled(
                format!("  {}", phase.name),
                theme.muted_style(),
            )));
        }
        for item in &phase.items {
            for (row, indent) in
                std::iter::once((item, "")).chain(item.children.iter().map(|child| (child, "  ")))
            {
                if shown.contains(&number) {
                    rows.push(todo_row(row, number.saturating_add(1), indent, theme));
                }
                number = number.saturating_add(1);
            }
        }
    }
    Some((title, rows))
}

fn todo_row(item: &TodoItem, number: usize, indent: &str, theme: &Theme) -> Line<'static> {
    let plain = Style::default().fg(theme.text);
    let (glyph, style) = match item.state {
        TodoStateName::Done => ("✓", theme.dim_style()),
        TodoStateName::Running => ("▶", theme.accent_style().add_modifier(Modifier::BOLD)),
        TodoStateName::Blocked => ("!", Style::default().fg(theme.warning)),
        TodoStateName::Abandoned | TodoStateName::Failed => {
            ("−", theme.dim_style().add_modifier(Modifier::CROSSED_OUT))
        }
        TodoStateName::Pending | TodoStateName::Other(_) => ("○", plain),
    };
    let cut = if item.is_cut() { "…" } else { "" };
    Line::from(Span::styled(
        format!(
            "  {indent}{number}. {glyph} {}{cut}{}",
            item.label,
            yi_runtime::todo::text::suffix(item)
        ),
        style,
    ))
}

pub(crate) fn input(
    app: &crate::app::App,
    goal: Option<GoalView>,
    memory: Option<String>,
) -> HudInput {
    HudInput {
        goal,
        landing: app.landing.as_ref().map(yi_runtime::slash::landing_line),
        plan: app.plan_progress.clone(),
        todos: app.todos.clone(),
        todo_full: app.todo_clock.full(Instant::now()),
        steering: app.steering.clone(),
        follow_up: Vec::new(),
        memory,
    }
}

/// The goal header over indented plan, todo, steering and follow-up rows.
pub fn render(input: &HudInput, theme: &Theme) -> Vec<Line<'static>> {
    let mut content: Vec<Line<'static>> = Vec::new();
    let header = match &input.goal {
        Some(goal) => {
            let mut header = format!("Goal {}", goal.objective);
            header.push_str(&format!(" · {}", goal.status));
            if let Some(budget) = goal.token_budget {
                header.push_str(&format!(
                    " · {}k/{}k",
                    goal.tokens_used / 1000,
                    budget / 1000
                ));
            }
            Some(header)
        }
        None => None,
    };
    let header = match (header, &input.plan) {
        (None, Some(plan)) => Some(plan.line()),
        (Some(header), Some(plan)) => {
            content.push(Line::from(Span::styled(plan.line(), theme.muted_style())));
            Some(header)
        }
        (header, None) => header,
    };
    let header = match (
        header,
        todo_rows(input.todos.as_ref(), input.todo_full, theme),
    ) {
        (header, None) => header,
        (None, Some((title, rows))) => {
            content.extend(rows);
            Some(title)
        }
        (Some(header), Some((title, rows))) => {
            content.push(Line::from(Span::styled(title, theme.muted_style())));
            content.extend(rows);
            Some(header)
        }
    };
    for (label, value) in [("Land", &input.landing), ("Memory", &input.memory)] {
        if let Some(value) = value {
            content.push(Line::from(Span::styled(
                format!("{label} · {value}"),
                theme.muted_style(),
            )));
        }
    }
    for (label, items) in [
        ("Steering", &input.steering),
        ("After yield", &input.follow_up),
    ] {
        if items.is_empty() {
            continue;
        }
        content.push(Line::from(Span::styled(
            format!("{label} · {}", items.len()),
            theme.muted_style(),
        )));
        for (i, item) in items.iter().enumerate() {
            let flat = item.replace('\n', " ↵ ");
            let mut flat = flat.trim().to_owned();
            if flat.chars().count() > 60 {
                flat = flat.chars().take(60).collect::<String>() + "…";
            }
            content.push(Line::from(Span::styled(
                format!("  {}. {flat}", i + 1),
                theme.dim_style(),
            )));
        }
    }

    if content.is_empty() && header.is_none() {
        return Vec::new();
    }

    let mut out = Vec::new();
    out.push(Line::from(Span::styled(
        format!(" {}", header.unwrap_or_default()),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    for line in content {
        let mut spans = vec![Span::raw("   ")];
        spans.extend(line.spans);
        out.push(Line::from(spans));
    }
    out
}
