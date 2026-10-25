use yi_types::message::{AgentMessage, UserContent};

use crate::account::{Tokens, estimate_message};

pub const RETENTION_FLOOR_BUDGET: Tokens = Tokens(64_000);

fn user_text(message: &AgentMessage) -> Option<&str> {
    match message {
        AgentMessage::User {
            content: UserContent::Text(text),
            ..
        } => Some(text),
        _ => None,
    }
}

fn middle_truncate(text: &str, budget: Tokens) -> String {
    let keep_chars = usize::try_from(budget.0.saturating_mul(4)).unwrap_or(usize::MAX);
    if text.len() <= keep_chars || keep_chars < 8 {
        return text.to_owned();
    }
    let half = keep_chars / 2;
    let head_end = text.floor_char_boundary(half);
    let tail_start = text.ceil_char_boundary(text.len().saturating_sub(half));
    let dropped = tail_start.saturating_sub(head_end);
    format!(
        "{}\n[... {dropped} characters truncated ...]\n{}",
        text.get(..head_end).unwrap_or_default(),
        text.get(tail_start..).unwrap_or_default()
    )
}

/// Every real user message survives compaction verbatim, newest first within the budget; the
/// oldest that partially fits is middle-truncated. Survivors come back in original order.
pub fn retain_floor(summarized: &[AgentMessage], budget: Tokens) -> Vec<AgentMessage> {
    let mut remaining = budget;
    let mut survivors_reversed: Vec<AgentMessage> = Vec::new();
    for message in summarized.iter().rev() {
        if remaining.0 == 0 {
            break;
        }
        let Some(text) = user_text(message) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        let cost = estimate_message(message);
        if cost <= remaining {
            survivors_reversed.push(message.clone());
            remaining = remaining.saturating_sub(cost);
        } else {
            let truncated = middle_truncate(text, remaining);
            let (timestamp, attribution) = match message {
                AgentMessage::User {
                    timestamp,
                    attribution,
                    ..
                } => (*timestamp, *attribution),
                _ => (0, yi_types::message::Attribution::Unproven),
            };
            survivors_reversed.push(AgentMessage::User {
                content: UserContent::Text(truncated),
                timestamp,
                attribution,
            });
            remaining = Tokens(0);
        }
    }
    survivors_reversed.reverse();
    survivors_reversed
}
