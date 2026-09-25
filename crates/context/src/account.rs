use yi_types::message::{AgentMessage, Content, StopReason, Usage, UserContent};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Tokens(pub u64);

impl Tokens {
    pub fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    pub fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

// Invariant: the most an image costs on Claude's high-resolution tier, 4784 visual tokens
// (a larger one is downscaled to that), at four chars a token.
const IMAGE_ESTIMATE_CHARS: u64 = 19_136;

fn chars_to_tokens(chars: u64) -> Tokens {
    Tokens(chars.div_ceil(4))
}

fn user_content_chars(content: &UserContent) -> u64 {
    match content {
        UserContent::Text(text) => text.len() as u64,
        UserContent::Blocks(blocks) => blocks.iter().map(content_chars).sum(),
    }
}

fn content_chars(block: &Content) -> u64 {
    match block {
        Content::Text { text, .. } => text.len() as u64,
        Content::Thinking { thinking, .. } => thinking.len() as u64,
        Content::Image { .. } => IMAGE_ESTIMATE_CHARS,
        Content::ToolCall {
            name, arguments, ..
        } => {
            let args = serde_json::to_string(arguments)
                .map(|json| json.len() as u64)
                .unwrap_or(0);
            name.len() as u64 + args
        }
    }
}

/// chars/4 heuristic, deliberately conservative (overestimates); the single
/// estimator for every context budget (D22).
pub fn estimate_message(message: &AgentMessage) -> Tokens {
    let chars = match message {
        AgentMessage::User { content, .. } => user_content_chars(content),
        AgentMessage::Assistant { content, .. } => content.iter().map(content_chars).sum(),
        AgentMessage::ToolResult { content, .. } => content.iter().map(content_chars).sum(),
        AgentMessage::Custom { content, .. } => user_content_chars(content),
        AgentMessage::BashExecution {
            command, output, ..
        } => command.len() as u64 + output.len() as u64,
        AgentMessage::BranchSummary { summary, .. }
        | AgentMessage::CompactionSummary { summary, .. } => summary.len() as u64,
    };
    chars_to_tokens(chars)
}

fn clamped(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// Total context tokens from a usage row: native total when present, else the
/// component sum. Output counts — the reply is part of the next request.
pub fn context_tokens(usage: &Usage) -> Tokens {
    if usage.total_tokens != 0 {
        return Tokens(clamped(usage.total_tokens));
    }
    let sum = usage
        .input
        .saturating_add(usage.output)
        .saturating_add(usage.cache_read)
        .saturating_add(usage.cache_write);
    Tokens(clamped(sum))
}

fn assistant_usage(message: &AgentMessage) -> Option<&Usage> {
    match message {
        AgentMessage::Assistant {
            usage, stop_reason, ..
        } if !matches!(stop_reason, StopReason::Aborted | StopReason::Error) => Some(usage),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Estimate {
    pub tokens: Tokens,
    pub usage_tokens: Tokens,
    pub trailing_tokens: Tokens,
    pub last_usage_index: Option<usize>,
}

/// Last authoritative assistant usage plus chars/4 for trailing messages.
pub fn estimate_context(messages: &[AgentMessage]) -> Estimate {
    let last = messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, message)| assistant_usage(message).map(|usage| (index, usage)));
    let Some((index, usage)) = last else {
        let estimated = messages
            .iter()
            .map(estimate_message)
            .fold(Tokens(0), Tokens::saturating_add);
        return Estimate {
            tokens: estimated,
            usage_tokens: Tokens(0),
            trailing_tokens: estimated,
            last_usage_index: None,
        };
    };
    let usage_tokens = context_tokens(usage);
    let trailing_tokens = messages[index.saturating_add(1)..]
        .iter()
        .map(estimate_message)
        .fold(Tokens(0), Tokens::saturating_add);
    Estimate {
        tokens: usage_tokens.saturating_add(trailing_tokens),
        usage_tokens,
        trailing_tokens,
        last_usage_index: Some(index),
    }
}

/// Which tokens count against the compaction budget (design P3): the whole context, or only
/// growth past the cached prefix — the ~10 %-priced prefix must not be charged full price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    Total,
    #[default]
    BodyAfterPrefix,
}

pub fn scoped_tokens(total: Tokens, scope: Scope, prefill: Option<Tokens>) -> Tokens {
    match scope {
        Scope::Total => total,
        Scope::BodyAfterPrefix => total.saturating_sub(prefill.unwrap_or(Tokens(0))),
    }
}
