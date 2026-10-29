use yi_types::message::{AgentMessage, UserContent};

pub const COMPACTION_SUMMARY_PREFIX: &str = "The conversation history before this point was compacted into the following summary:\n\n<summary>\n";
pub const COMPACTION_SUMMARY_SUFFIX: &str = "\n</summary>";
pub const BRANCH_SUMMARY_PREFIX: &str =
    "The following is a summary of a branch that this conversation came back from:\n\n<summary>\n";
pub const BRANCH_SUMMARY_SUFFIX: &str = "</summary>";

fn bash_execution_to_text(
    command: &str,
    output: &str,
    exit_code: Option<i64>,
    cancelled: bool,
    truncated: bool,
    full_output_path: Option<&str>,
) -> String {
    let mut text = format!("Ran `{command}`\n");
    if output.is_empty() {
        text.push_str("(no output)");
    } else {
        text.push_str(&format!("```\n{output}\n```"));
    }
    if cancelled {
        text.push_str("\n\n(command cancelled)");
    } else if let Some(code) = exit_code
        && code != 0
    {
        text.push_str(&format!("\n\nCommand exited with code {code}"));
    }
    if truncated && let Some(path) = full_output_path {
        text.push_str(&format!("\n\n[Output truncated. Full output: {path}]"));
    }
    text
}

/// Invariant: only its requester's words reach the model as bare user text; every other
/// user-role or custom message arrives fenced as runtime context under its source.
fn host_view(source: &str, content: &UserContent, timestamp: u64) -> AgentMessage {
    crate::wrapper::wrap_content(source, content, timestamp)
}

/// Design §4.2: harness-internal message kinds become user-role messages, fenced unless they
/// are the requester's words; kinds with no LLM representation drop.
pub fn convert_to_llm(messages: &[AgentMessage]) -> Vec<AgentMessage> {
    let _span = yi_types::trace::span("context.convert_to_llm").arg("messages", messages.len());
    let last_reply = messages
        .iter()
        .rposition(|message| matches!(message, AgentMessage::Assistant { .. }));
    messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            let read = last_reply.is_some_and(|reply| reply > index);
            match message {
                AgentMessage::BashExecution {
                    command,
                    output,
                    exit_code,
                    cancelled,
                    truncated,
                    full_output_path,
                    timestamp,
                    exclude_from_context,
                } => {
                    if exclude_from_context.unwrap_or(false) {
                        return None;
                    }
                    let text = bash_execution_to_text(
                        command,
                        output,
                        *exit_code,
                        *cancelled,
                        *truncated,
                        full_output_path.as_deref(),
                    );
                    Some(AgentMessage::user_input(
                        UserContent::Text(text),
                        *timestamp,
                    ))
                }
                AgentMessage::Custom {
                    custom_type,
                    content,
                    timestamp,
                    ..
                } => {
                    if superseded(index, custom_type, messages) {
                        return None;
                    }
                    let content = read_content(custom_type, content, read);
                    match crate::wrapper::internal_source_of_custom(custom_type) {
                        Some(source) => Some(crate::wrapper::wrap_internal(
                            source,
                            match &content {
                                UserContent::Text(text) => text,
                                UserContent::Blocks(_) => "",
                            },
                            *timestamp,
                        )),
                        None => Some(host_view(custom_type, &content, *timestamp)),
                    }
                }
                AgentMessage::BranchSummary {
                    summary, timestamp, ..
                } => Some(crate::wrapper::wrap_internal(
                    "summary",
                    &format!("{BRANCH_SUMMARY_PREFIX}{summary}{BRANCH_SUMMARY_SUFFIX}"),
                    *timestamp,
                )),
                AgentMessage::CompactionSummary {
                    summary, timestamp, ..
                } => Some(crate::wrapper::wrap_internal(
                    "summary",
                    &format!("{COMPACTION_SUMMARY_PREFIX}{summary}{COMPACTION_SUMMARY_SUFFIX}"),
                    *timestamp,
                )),
                AgentMessage::User {
                    content,
                    timestamp,
                    attribution,
                } => Some(match attribution {
                    yi_types::message::Attribution::Host(source) => {
                        host_view(source.label(), content, *timestamp)
                    }
                    _ if attribution.reads_as_typed(*timestamp) => message.clone(),
                    _ if crate::wrapper::internal_source(message).is_some() => message.clone(),
                    _ => host_view("host", content, *timestamp),
                }),
                AgentMessage::Assistant { .. } | AgentMessage::ToolResult { .. } => {
                    Some(message.clone())
                }
            }
        })
        .collect()
}

fn superseded(index: usize, custom_type: &str, messages: &[AgentMessage]) -> bool {
    crate::wrapper::superseded_by_kind(custom_type)
        && messages.get(index.saturating_add(1)..).is_some_and(|later| {
            later.iter().any(|message| {
                matches!(message, AgentMessage::Custom { custom_type: other, .. } if other == custom_type)
            })
        })
}

fn read_content(custom_type: &str, content: &UserContent, read: bool) -> UserContent {
    match content {
        UserContent::Text(text) if read && crate::wrapper::quote_until_next_turn(custom_type) => {
            UserContent::Text(crate::wrapper::without_quote(text))
        }
        other => other.clone(),
    }
}
