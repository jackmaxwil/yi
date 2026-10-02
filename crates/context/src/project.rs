use yi_types::entry::Entry;
use yi_types::message::{ATTRIBUTED_SINCE_MS, AgentMessage, Attribution, StopReason};

use crate::view::Attributed;

/// Invariant: a user message Yi stored before attribution existed stamps 0; the entry's own time
/// dates it, so the converter reads those words as typed and fences what came after.
fn dated(message: &AgentMessage, stored_ms: u64) -> AgentMessage {
    match message {
        AgentMessage::User {
            content,
            timestamp: 0,
            attribution: attribution @ Attribution::Unproven,
        } if stored_ms < ATTRIBUTED_SINCE_MS => AgentMessage::User {
            content: content.clone(),
            timestamp: stored_ms,
            attribution: *attribution,
        },
        _ => message.clone(),
    }
}

fn entry_attributed(entry: &Entry) -> Vec<Attributed> {
    let id = entry.id().to_owned();
    match entry {
        Entry::Message {
            message, timestamp, ..
        } => match message {
            AgentMessage::Assistant { stop_reason, .. } if *stop_reason == StopReason::Deferred => {
                Vec::new()
            }
            _ => vec![Attributed {
                id: Some(id),
                message: dated(message, *timestamp),
            }],
        },
        Entry::Compaction {
            summary,
            retained_tail,
            tokens_before,
            timestamp,
            ..
        } => {
            let mut messages = Vec::with_capacity(retained_tail.len().saturating_add(1));
            messages.push(Attributed {
                id: Some(id),
                message: AgentMessage::CompactionSummary {
                    summary: summary.clone(),
                    tokens_before: *tokens_before,
                    timestamp: *timestamp,
                },
            });
            // Invariant: a retained-tail message carries no id. The compaction's id would
            // send `history://` to the summary, not the turn the pointer promised.
            messages.extend(
                retained_tail
                    .iter()
                    .cloned()
                    .map(|message| Attributed { id: None, message }),
            );
            messages
        }
        Entry::BranchSummary {
            summary,
            from_id,
            timestamp,
            ..
        } if !summary.is_empty() => vec![Attributed {
            id: Some(id),
            message: AgentMessage::BranchSummary {
                summary: summary.clone(),
                from_id: from_id.clone(),
                timestamp: *timestamp,
            },
        }],
        _ => Vec::new(),
    }
}

/// Same slice as [`project`], with the producing entry id on each message.
pub fn project_attributed(branch: &[Entry]) -> Vec<Attributed> {
    let _span = yi_types::trace::span("context.project").arg("entries", branch.len());
    let start = branch
        .iter()
        .rposition(|entry| matches!(entry, Entry::Compaction { .. }))
        .unwrap_or(0);
    branch[start..].iter().flat_map(entry_attributed).collect()
}

/// Design §4.4: drops non-message entries and applies the latest compaction — everything before
/// it becomes that summary plus its retained tail (Pi v4 session format semantics).
pub fn project(branch: &[Entry]) -> Vec<AgentMessage> {
    project_attributed(branch)
        .into_iter()
        .map(|entry| entry.message)
        .collect()
}
