use yi_types::message::{AgentMessage, Attribution, Content, UserContent};

// Tool results are truncated in serialized summaries; full content is not
// needed for summarization.
const TOOL_RESULT_MAX_CHARS: usize = 2000;

fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_owned();
    }
    let mut end = max_chars;
    while end > 0 && !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    let truncated_chars = text.len().saturating_sub(end);
    format!(
        "{}\n\n[... {truncated_chars} more characters truncated]",
        &text[..end]
    )
}

pub(crate) fn text_of(blocks: &[Content]) -> String {
    yi_types::message::join_text(blocks, "")
}

fn user_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => text_of(blocks),
    }
}

fn push_assistant(parts: &mut Vec<String>, content: &[Content]) {
    let mut texts = Vec::new();
    let mut thinking = Vec::new();
    let mut calls = Vec::new();
    for block in content {
        match block {
            Content::Text { text, .. } => texts.push(text.clone()),
            Content::Thinking {
                thinking: thought, ..
            } => thinking.push(thought.clone()),
            Content::ToolCall {
                name, arguments, ..
            } => {
                let args = arguments
                    .iter()
                    .map(|(key, value)| {
                        format!("{key}={}", serde_json::to_string(value).unwrap_or_default())
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                calls.push(format!("{name}({args})"));
            }
            Content::Image { .. } => {}
        }
    }
    if !thinking.is_empty() {
        parts.push(format!("[Assistant thinking]: {}", thinking.join("\n")));
    }
    if !texts.is_empty() {
        parts.push(format!("[Assistant]: {}", texts.join("\n")));
    }
    if !calls.is_empty() {
        parts.push(format!("[Assistant tool calls]: {}", calls.join("; ")));
    }
}

/// Design §4.4: flatten a conversation to labeled text so the summarizer reads
/// it as material, never as a conversation to continue.
pub fn serialize_conversation(messages: &[AgentMessage]) -> String {
    let _span = yi_types::trace::span("context.serialize").arg("messages", messages.len());
    let mut parts: Vec<String> = Vec::new();
    for message in messages {
        match message {
            AgentMessage::User { content, .. } | AgentMessage::Custom { content, .. } => {
                let text = user_text(content);
                if !text.is_empty() {
                    parts.push(format!("[User]: {text}"));
                }
            }
            AgentMessage::Assistant { content, .. } => push_assistant(&mut parts, content),
            AgentMessage::ToolResult { content, .. } => {
                let text = text_of(content);
                if !text.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate_for_summary(&text, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
            AgentMessage::BashExecution {
                command, output, ..
            } => {
                parts.push(format!("[User]: $ {command}"));
                if !output.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate_for_summary(output, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
            AgentMessage::BranchSummary { summary, .. }
            | AgentMessage::CompactionSummary { summary, .. } => {
                if !summary.is_empty() {
                    parts.push(format!("[User]: {summary}"));
                }
            }
        }
    }
    parts.join("\n\n")
}

/// Characters of each message the key quotes, enough to tell one message from the next.
pub const KEY_HEAD_CHARS: usize = 60;

/// Rows the key keeps, the newest: a long session's one-line asks would otherwise spend the
/// summarizer's reserve on the key, and a request that overflows twice summarizes nothing.
pub const KEY_ROWS: usize = 100;

/// The summarizer's key from each typed message in `window` to its `user://<n>`, `inputs[n-1]`.
/// ponytail: matched by content in order, so of two identical messages the first address wins.
pub fn user_key(window: &[AgentMessage], inputs: &[UserContent]) -> String {
    let mut rows = Vec::new();
    let mut from = 0usize;
    for message in window {
        let AgentMessage::User {
            content,
            attribution: Attribution::User,
            ..
        } = message
        else {
            continue;
        };
        let Some(at) = inputs.iter().skip(from).position(|input| input == content) else {
            continue;
        };
        let ordinal = from.saturating_add(at).saturating_add(1);
        from = ordinal;
        let text = user_text(content);
        let total = text.chars().count();
        let head: String = text.chars().take(KEY_HEAD_CHARS).collect();
        let cut = if total > KEY_HEAD_CHARS {
            format!(" [… {KEY_HEAD_CHARS} of {total} chars]")
        } else {
            String::new()
        };
        rows.push(format!("user://{ordinal}: {head:?}{cut}"));
    }
    if rows.is_empty() {
        return String::new();
    }
    let older = rows.len().saturating_sub(KEY_ROWS);
    let dropped = (older > 0).then(|| {
        format!(
            "[… {KEY_ROWS} of {} messages keyed, the newest (key cap {KEY_ROWS}); quote an older one verbatim from the conversation above instead of citing it]",
            rows.len()
        )
    });
    let rows: Vec<String> = dropped
        .into_iter()
        .chain(rows.into_iter().skip(older))
        .collect();
    format!(
        "<user-messages>\nThe user's own messages above, by address; each quote is at most its first {KEY_HEAD_CHARS} characters and the whole message is in the conversation above.\n{}\n</user-messages>\n\n",
        rows.join("\n")
    )
}
