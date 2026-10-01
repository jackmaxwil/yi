use yi_types::message::AgentMessage;

use crate::account::{Tokens, estimate_message};

/// Any message but a tool result, which must follow its call.
pub fn is_cut_point(message: &AgentMessage) -> bool {
    !matches!(message, AgentMessage::ToolResult { .. })
}

fn is_turn_start(message: &AgentMessage) -> bool {
    matches!(
        message,
        AgentMessage::User { .. }
            | AgentMessage::BashExecution { .. }
            | AgentMessage::Custom { .. }
            | AgentMessage::BranchSummary { .. }
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cut {
    pub first_kept_index: usize,
    pub turn_start_index: Option<usize>,
    pub is_split_turn: bool,
}

/// Cuts at the nearest valid point at or after `keep_recent`, never at a tool result. A cut
/// inside a non-user turn is a split turn and records the turn's starting user message.
pub fn select_cut(messages: &[AgentMessage], keep_recent: Tokens) -> Cut {
    let _span = yi_types::trace::span("context.select_cut").arg("messages", messages.len());
    let Some(first_cut) = messages.iter().position(is_cut_point) else {
        return Cut {
            first_kept_index: 0,
            turn_start_index: None,
            is_split_turn: false,
        };
    };
    let mut cut_index = first_cut;
    let mut accumulated = Tokens(0);
    for (index, message) in messages.iter().enumerate().rev() {
        accumulated = accumulated.saturating_add(estimate_message(message));
        if accumulated >= keep_recent {
            cut_index = messages[index..]
                .iter()
                .position(is_cut_point)
                .map(|offset| index.saturating_add(offset))
                .unwrap_or(first_cut);
            break;
        }
    }
    let is_user = matches!(messages.get(cut_index), Some(AgentMessage::User { .. }));
    let turn_start_index = if is_user {
        None
    } else {
        messages[..cut_index].iter().rposition(is_turn_start)
    };
    Cut {
        first_kept_index: cut_index,
        turn_start_index,
        is_split_turn: !is_user && turn_start_index.is_some(),
    }
}
