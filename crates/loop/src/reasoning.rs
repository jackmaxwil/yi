//! The reasoning-char budget of one request (D163), calibrated on rows 0018-0023: healthy
//! blocks p99 23k chars (seven over 48k), spirals min 72k; omp's rules tripped none of 29.

use yi_types::message::{AgentMessage, Content};

/// Chars of reasoning a request may stream before any text or tool call.
pub const REASONING_CHAR_CAP: usize = 48_000;

/// Chars of a cut turn's reasoning its re-drive quotes: nine in ten of the 2026-09-11 sweep's
/// 1,455 thinking blocks that ended in a tool call were shorter (p90 7,594).
pub(crate) const CUT_QUOTE_CHARS: usize = 8_000;

/// The last [`CUT_QUOTE_CHARS`] chars of a turn's reasoning, across its thinking blocks.
pub(crate) fn tail(message: &AgentMessage) -> String {
    let AgentMessage::Assistant { content, .. } = message else {
        return String::new();
    };
    let thinking: String = content
        .iter()
        .filter_map(|block| {
            if let Content::Thinking { thinking, .. } = block {
                Some(thinking.as_str())
            } else {
                None
            }
        })
        .collect();
    let skip = thinking.chars().count().saturating_sub(CUT_QUOTE_CHARS);
    thinking.chars().skip(skip).collect()
}

/// Counts reasoning deltas until text or a tool call disarms it.
#[derive(Debug, Default)]
pub struct ReasoningBudget {
    chars: usize,
    disarmed: bool,
}

impl ReasoningBudget {
    /// The char count when this delta crossed the cap; `None` while under it or disarmed.
    pub fn push(&mut self, delta: &str) -> Option<usize> {
        if self.disarmed {
            return None;
        }
        let before = self.chars;
        self.chars = self.chars.saturating_add(delta.chars().count());
        (before < REASONING_CHAR_CAP && self.chars >= REASONING_CHAR_CAP).then_some(self.chars)
    }

    pub fn disarm(&mut self) {
        self.disarmed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::{REASONING_CHAR_CAP, ReasoningBudget};

    #[test]
    fn the_budget_trips_once_at_the_cap_and_never_after_text_starts() {
        let mut budget = ReasoningBudget::default();
        let chunk = "x".repeat(REASONING_CHAR_CAP / 2);
        assert_eq!(budget.push(&chunk), None);
        assert_eq!(budget.push(&chunk), Some(REASONING_CHAR_CAP));
        assert_eq!(budget.push("more"), None, "once");
        let mut armed = ReasoningBudget::default();
        armed.disarm();
        assert_eq!(armed.push(&"y".repeat(REASONING_CHAR_CAP)), None);
    }
}
