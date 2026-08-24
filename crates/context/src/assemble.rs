use yi_types::message::AgentMessage;

use crate::wrapper::wrap_internal;

/// Design P11 stable prefix: pieces whose bytes must not change within a
/// window — the system prompt and the ledger fragment. The compaction summary
/// rides the message list (it is itself a message), and world-state diffs are
/// appended at the overlay tail so the prefix stays cache-warm.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StablePrefix {
    pub system_prompt: String,
    pub ledger: Option<String>,
}

impl StablePrefix {
    pub fn system_text(&self) -> String {
        match &self.ledger {
            Some(ledger) => format!("{}\n\n{ledger}", self.system_prompt),
            None => self.system_prompt.clone(),
        }
    }
}

/// Appends world-state overlay fragments to the kept messages as wrapped
/// internal context — append-only diffs at the tail, never edits to earlier
/// messages.
pub fn assemble(kept: &[AgentMessage], overlay: &[String], timestamp: u64) -> Vec<AgentMessage> {
    let mut messages = kept.to_vec();
    for fragment in overlay {
        messages.push(wrap_internal("world_state", fragment, timestamp));
    }
    messages
}
