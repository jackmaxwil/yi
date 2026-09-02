use yi_types::message::{AgentMessage, UserContent};

fn valid_source(source: &str) -> bool {
    let mut chars = source.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

/// The wrapper makes an injected prompt data-with-provenance to the model and
/// droppable at compaction, so injected context never accumulates across
/// windows. Invalid source labels fall back to `internal`.
pub fn wrap_internal(source: &str, text: &str, timestamp: u64) -> AgentMessage {
    let source = if valid_source(source) {
        source
    } else {
        "internal"
    };
    AgentMessage::host_user(
        UserContent::Text(format!(
            "<yi_internal_context source=\"{source}\">\n{text}\n</yi_internal_context>"
        )),
        timestamp,
    )
}

pub fn internal_source(message: &AgentMessage) -> Option<&str> {
    let AgentMessage::User {
        content: UserContent::Text(text),
        ..
    } = message
    else {
        return None;
    };
    let rest = text.strip_prefix("<yi_internal_context source=\"")?;
    let end = rest.find('"')?;
    text.ends_with("</yi_internal_context>")
        .then_some(&rest[..end])
}

/// Custom entry kinds that ride the L4 wrapper: their LLM rendering is
/// `<yi_internal_context source="…">` and they are dropped at compaction so
/// injected prompts never accumulate across windows.
pub fn internal_source_of_custom(custom_type: &str) -> Option<&'static str> {
    match custom_type {
        "heartbeat_prompt" => Some("heartbeat"),
        "advisory" => Some("advisory"),
        "goal_prompt" => Some("goal"),
        "ledger_prompt" => Some("ledger"),
        "plan_dispatch" => Some("dispatch"),
        "reminder" => Some("reminder"),
        _ => None,
    }
}

/// Removes wrapped internal-context messages — applied to the region being
/// summarized so per-window injections die with their window.
pub fn drop_internal(messages: &[AgentMessage]) -> Vec<AgentMessage> {
    messages
        .iter()
        .filter(|message| {
            if let AgentMessage::Custom { custom_type, .. } = message {
                return internal_source_of_custom(custom_type).is_none();
            }
            internal_source(message).is_none()
        })
        .cloned()
        .collect()
}
