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

fn as_user(text: String, timestamp: u64) -> AgentMessage {
    AgentMessage::User {
        content: UserContent::Text(text),
        timestamp,
    }
}

/// Design L4 (Pi `convertToLlm`, ported verbatim in text): harness-internal
/// message kinds become plain user messages the provider adapters understand;
/// kinds with no LLM representation drop.
pub fn convert_to_llm(messages: &[AgentMessage]) -> Vec<AgentMessage> {
    messages
        .iter()
        .filter_map(|message| match message {
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
                Some(as_user(
                    bash_execution_to_text(
                        command,
                        output,
                        *exit_code,
                        *cancelled,
                        *truncated,
                        full_output_path.as_deref(),
                    ),
                    *timestamp,
                ))
            }
            AgentMessage::Custom {
                custom_type,
                content,
                timestamp,
                ..
            } => match crate::wrapper::internal_source_of_custom(custom_type) {
                Some(source) => Some(crate::wrapper::wrap_internal(
                    source,
                    match content {
                        yi_types::message::UserContent::Text(text) => text,
                        yi_types::message::UserContent::Blocks(_) => "",
                    },
                    *timestamp,
                )),
                None => Some(AgentMessage::User {
                    content: content.clone(),
                    timestamp: *timestamp,
                }),
            },
            AgentMessage::BranchSummary {
                summary, timestamp, ..
            } => Some(as_user(
                format!("{BRANCH_SUMMARY_PREFIX}{summary}{BRANCH_SUMMARY_SUFFIX}"),
                *timestamp,
            )),
            AgentMessage::CompactionSummary {
                summary, timestamp, ..
            } => Some(as_user(
                format!("{COMPACTION_SUMMARY_PREFIX}{summary}{COMPACTION_SUMMARY_SUFFIX}"),
                *timestamp,
            )),
            AgentMessage::User { .. }
            | AgentMessage::Assistant { .. }
            | AgentMessage::ToolResult { .. } => Some(message.clone()),
        })
        .collect()
}
