//! Tokens and cost per message and per thread.

use yi_types::message::AgentMessage;
use yi_types::wire::SessionStats;

/// A count of tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Tokens(pub u64);

/// What a thread has used. A message's own usage rides in the message.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Usage {
    /// What the context holds now.
    pub used: Tokens,
    /// The model's context window.
    pub window: Tokens,
    /// The thread's totals as the server's ledger gave them at the last replay. They are
    /// set, never added to: the ledger already counts every reply it has stored.
    pub ledger: Option<SessionStats>,
}

/// What the replies an agent finished while this app watched have cost.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Spend {
    pub replies: u64,
    pub tokens: Tokens,
    /// Dollars.
    pub cost: f64,
    /// A reply came without usage, so the cost is a lower bound.
    pub unknown: bool,
}

impl Spend {
    /// Counts a message that just became whole; only an assistant's reply costs anything.
    pub(crate) fn add(&mut self, message: &AgentMessage) {
        let AgentMessage::Assistant { usage, .. } = message else {
            return;
        };
        self.replies = self.replies.saturating_add(1);
        self.tokens = Tokens(self.tokens.0.saturating_add(usage.fresh()));
        self.cost += usage.cost.total.as_f64().unwrap_or(0.0);
        self.unknown |= usage.unknown;
    }
}
