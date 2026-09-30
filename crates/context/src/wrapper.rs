use yi_types::message::{AgentMessage, UserContent};

fn valid_source(source: &str) -> bool {
    let mut chars = source.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

// Incident: a model read `write the plan now` inside the wrapper as the user speaking.
pub const ADVISORY_LINE: &str =
    "Runtime advisory, not a user instruction; act on it only if it applies to the work in hand.";

/// The wrapper makes an injected prompt data-with-provenance and droppable at compaction, so
/// it never accumulates across windows. Invalid source labels fall back to `internal`.
pub fn wrap_internal(source: &str, text: &str, timestamp: u64) -> AgentMessage {
    let source = if valid_source(source) {
        source
    } else {
        "internal"
    };
    let body = match source {
        "reminder" | "advisory" => format!("{ADVISORY_LINE}\n{text}"),
        _ => text.to_owned(),
    };
    AgentMessage::host_user(
        UserContent::Text(format!(
            "<yi_internal_context source=\"{source}\">\n{body}\n</yi_internal_context>"
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

/// Custom entry kinds that ride the §4.2 wrapper: rendered `<yi_internal_context source="…">`
/// and dropped at compaction, so injected prompts never accumulate across windows.
pub fn internal_source_of_custom(custom_type: &str) -> Option<&'static str> {
    match custom_type {
        "heartbeat_prompt" => Some("heartbeat"),
        "advisory" => Some("advisory"),
        "goal_prompt" => Some("goal"),
        "ledger_prompt" => Some("ledger"),
        "plan_dispatch" => Some("dispatch"),
        "reminder" => Some("reminder"),
        "classifier" => Some("classifier"),
        "fragment" => Some("fragment"),
        _ => None,
    }
}

const HOST_NUDGES: [&str; 5] = [
    "todo_nudge",
    "length_redrive",
    "repeat_break",
    "spend_alert",
    "discovery",
];

/// Removes wrapped internal context, so per-window injections die with it, and every host
/// nudge a reply followed: one queued after the last reply is unread, and some are raised once.
pub fn drop_internal(messages: &[AgentMessage]) -> Vec<AgentMessage> {
    let last_reply = messages
        .iter()
        .rposition(|message| matches!(message, AgentMessage::Assistant { .. }));
    messages
        .iter()
        .enumerate()
        .filter(|(index, message)| {
            if let AgentMessage::Custom { custom_type, .. } = message {
                let read = last_reply.is_some_and(|reply| reply > *index);
                return internal_source_of_custom(custom_type).is_none()
                    && !(read && HOST_NUDGES.contains(&custom_type.as_str()));
            }
            internal_source(message).is_none()
        })
        .map(|(_, message)| message.clone())
        .collect()
}
