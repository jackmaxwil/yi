use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::colors::Theme;

#[derive(Debug, Clone, Default)]
pub struct HudInput {
    pub goal: Option<GoalView>,
    pub plan: Option<PlanProgress>,
    pub steering: Vec<String>,
    pub follow_up: Vec<String>,
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

const TAIL_LEN: usize = 4;

/// The goal header over a dim tree spine of steering and follow-up rows.
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
        let mut spans = vec![Span::styled(" ├─ ", theme.dim_style())];
        spans.extend(line.spans);
        out.push(Line::from(spans));
    }
    let tail_fill: String = std::iter::repeat_n('─', TAIL_LEN).collect();
    out.push(Line::from(Span::styled(
        format!(" └{tail_fill}"),
        theme.dim_style(),
    )));
    out
}
