use yi_types::message::{AgentMessage, UserContent};

fn valid_source(source: &str) -> bool {
    let mut chars = source.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

/// Design L4: injected prompts ride a recognizable wrapper so they are data
/// with provenance to the model and are dropped at compaction — injected
/// context never accumulates across windows. Invalid source labels fall back
/// to `internal`.
pub fn wrap_internal(source: &str, text: &str, timestamp: u64) -> AgentMessage {
    let source = if valid_source(source) {
        source
    } else {
        "internal"
    };
    AgentMessage::User {
        content: UserContent::Text(format!(
            "<yi_internal_context source=\"{source}\">\n{text}\n</yi_internal_context>"
        )),
        timestamp,
    }
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

/// Removes wrapped internal-context messages — applied to the region being
/// summarized so per-window injections die with their window.
pub fn drop_internal(messages: &[AgentMessage]) -> Vec<AgentMessage> {
    messages
        .iter()
        .filter(|message| internal_source(message).is_none())
        .cloned()
        .collect()
}
