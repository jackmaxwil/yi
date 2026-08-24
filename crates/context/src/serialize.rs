use yi_types::message::{AgentMessage, Content, UserContent};

// Prime `utils.ts`: tool results are truncated in serialized summaries; full
// content is not needed for summarization.
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

fn text_of(blocks: &[Content]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
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

/// Design P6: flatten a conversation to labeled text so the summarizer reads
/// it as material, never as a conversation to continue.
pub fn serialize_conversation(messages: &[AgentMessage]) -> String {
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
