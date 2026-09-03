use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, StopReason};

fn entry_messages(entry: &Entry) -> Vec<AgentMessage> {
    match entry {
        Entry::Message { message, .. } => match message {
            AgentMessage::Assistant { stop_reason, .. } if *stop_reason == StopReason::Deferred => {
                Vec::new()
            }
            _ => vec![message.clone()],
        },
        Entry::Compaction {
            summary,
            retained_tail,
            tokens_before,
            timestamp,
            ..
        } => {
            let mut messages = Vec::with_capacity(retained_tail.len().saturating_add(1));
            messages.push(AgentMessage::CompactionSummary {
                summary: summary.clone(),
                tokens_before: *tokens_before,
                timestamp: *timestamp,
            });
            messages.extend(retained_tail.iter().cloned());
            messages
        }
        Entry::BranchSummary {
            summary,
            from_id,
            timestamp,
            ..
        } if !summary.is_empty() => vec![AgentMessage::BranchSummary {
            summary: summary.clone(),
            from_id: from_id.clone(),
            timestamp: *timestamp,
        }],
        _ => Vec::new(),
    }
}

/// Design P2: drops non-message entries and applies the latest compaction — everything before
/// it becomes that summary plus its retained tail (Pi v4 `context.ts` semantics).
pub fn project(branch: &[Entry]) -> Vec<AgentMessage> {
    let start = branch
        .iter()
        .rposition(|entry| matches!(entry, Entry::Compaction { .. }))
        .unwrap_or(0);
    branch[start..].iter().flat_map(entry_messages).collect()
}
