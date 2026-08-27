use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::cell::spinner_frame;
use crate::colors::{Theme, name_accent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    Heartbeat,
    Subagent,
    FollowUp,
    Task,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardStatus {
    Todo,
    Running,
    Blocked,
    Done,
}

/// U20: a view over live state, nothing persisted.
#[derive(Debug, Clone)]
pub struct BoardCard {
    pub title: String,
    pub kind: CardKind,
    pub status: CardStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct HudInput {
    pub goal: Option<GoalView>,
    pub cards: Vec<BoardCard>,
    pub steering: Vec<String>,
    pub follow_up: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GoalView {
    pub objective: String,
    pub status: String,
    pub tokens_used: u64,
    pub token_budget: Option<u64>,
}

const VISIBLE_LIMIT: usize = 8;
const TAIL_LEN: usize = 4;

fn card_row(card: &BoardCard, theme: &Theme, spinner_phase: usize) -> Line<'static> {
    let (glyph, style) = match card.status {
        CardStatus::Running => (
            spinner_frame(spinner_phase),
            Style::default().fg(name_accent(&card.title)),
        ),
        CardStatus::Todo => ('☐', theme.dim_style()),
        CardStatus::Blocked => ('☐', Style::default().fg(theme.warning)),
        CardStatus::Done => (
            '☑',
            Style::default()
                .fg(theme.success)
                .add_modifier(Modifier::CROSSED_OUT),
        ),
    };
    let text = if card.detail.is_empty() {
        format!("{glyph} {}", card.title)
    } else {
        format!("{glyph} {}: {}", card.title, card.detail)
    };
    Line::from(Span::styled(text, style))
}

/// The tree-spine connector is the progress meter, lit accent top-down by
/// done/total: at least one cell on any progress, never full until all done.
pub fn render(input: &HudInput, theme: &Theme, spinner_phase: usize) -> Vec<Line<'static>> {
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
        None if !input.cards.is_empty() => Some("Subagents".to_owned()),
        None => None,
    };
    let visible = input.cards.iter().take(VISIBLE_LIMIT);
    for card in visible {
        content.push(card_row(card, theme, spinner_phase));
    }
    if input.cards.len() > VISIBLE_LIMIT {
        content.push(Line::from(Span::styled(
            format!("… {} more", input.cards.len() - VISIBLE_LIMIT),
            theme.muted_style(),
        )));
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

    let total = input.cards.len();
    let closed = input
        .cards
        .iter()
        .filter(|c| c.status == CardStatus::Done)
        .count();
    let path_len = content.len() + TAIL_LEN;
    let mut filled = if total == 0 {
        0
    } else {
        (closed * path_len + total / 2) / total
    };
    if closed > 0 {
        filled = filled.max(1);
    }
    if closed < total {
        filled = filled.min(path_len.saturating_sub(1));
    }

    let mut out = Vec::new();
    out.push(Line::from(Span::styled(
        format!(" {}", header.unwrap_or_default()),
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
    )));
    for (i, line) in content.into_iter().enumerate() {
        let lit = i < filled;
        let spine_style = if lit {
            Style::default().fg(theme.accent)
        } else {
            theme.dim_style()
        };
        let mut spans = vec![Span::styled(" ├─ ", spine_style)];
        spans.extend(line.spans);
        out.push(Line::from(spans));
    }
    let tail_fill: String = std::iter::repeat_n('─', TAIL_LEN).collect();
    let tail_lit = filled >= path_len;
    out.push(Line::from(Span::styled(
        format!(" └{tail_fill}"),
        if tail_lit {
            Style::default().fg(theme.accent)
        } else {
            theme.dim_style()
        },
    )));
    out
}
