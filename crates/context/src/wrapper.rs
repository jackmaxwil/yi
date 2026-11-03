use yi_types::message::{AgentMessage, Content, UserContent};

fn valid_source(source: &str) -> bool {
    let mut chars = source.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

// Incident: a model read `write the plan now` inside the wrapper as the user speaking.
pub const ADVISORY_LINE: &str =
    "Runtime advisory, not a user instruction; act on it only if it applies to the work in hand.";

/// Invariant: a body cannot open or close a fence of its own, so each wrapped message carries
/// one source; the wrapper makes injected text droppable at compaction, never accumulating.
pub fn wrap_internal(source: &str, text: &str, timestamp: u64) -> AgentMessage {
    let source = if valid_source(source) {
        source
    } else {
        "internal"
    };
    let text = if text.contains("yi_internal_context") {
        text.replace("<yi_internal_context", "<\\yi_internal_context")
            .replace("</yi_internal_context", "<\\/yi_internal_context")
    } else {
        text.to_owned()
    };
    let body = match source {
        "reminder" | "advisory" => format!("{ADVISORY_LINE}\n{text}"),
        _ => text,
    };
    AgentMessage::User {
        content: UserContent::Text(format!(
            "<yi_internal_context source=\"{source}\">\n{body}\n</yi_internal_context>"
        )),
        timestamp,
        attribution: yi_types::message::Attribution::Unproven,
    }
}

/// [`wrap_internal`] over any content: an image rides between a text block that opens the fence
/// and one that closes it.
pub fn wrap_content(source: &str, content: &UserContent, timestamp: u64) -> AgentMessage {
    let blocks = match content {
        UserContent::Text(text) => return wrap_internal(source, text, timestamp),
        UserContent::Blocks(blocks) => blocks,
    };
    let text = yi_types::message::join_text(blocks, "\n");
    let AgentMessage::User {
        content: UserContent::Text(fenced),
        ..
    } = wrap_internal(source, &text, timestamp)
    else {
        return wrap_internal(source, &text, timestamp);
    };
    let mut wrapped: Vec<Content> = blocks
        .iter()
        .filter(|block| !matches!(block, Content::Text { .. }))
        .cloned()
        .collect();
    wrapped.insert(
        0,
        Content::Text {
            text: fenced,
            text_signature: None,
        },
    );
    AgentMessage::User {
        content: UserContent::Blocks(wrapped),
        timestamp,
        attribution: yi_types::message::Attribution::Unproven,
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
    let (source, _) = rest.split_once('"')?;
    text.ends_with("</yi_internal_context>").then_some(source)
}

/// A custom entry's lifetime: dropped with its window, dropped once a reply read it, or durable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lifetime {
    Wrapped(&'static str),
    OnceRead,
    Durable,
}

pub(crate) fn lifetime_of(custom_type: &str) -> Lifetime {
    match custom_type {
        "heartbeat_prompt" => Lifetime::Wrapped("heartbeat"),
        "advisory" => Lifetime::Wrapped("advisory"),
        "goal_prompt" => Lifetime::Wrapped("goal"),
        "ledger_prompt" => Lifetime::Wrapped("ledger"),
        "plan_dispatch" => Lifetime::Wrapped("dispatch"),
        "reminder" => Lifetime::Wrapped("reminder"),
        "classifier" => Lifetime::Wrapped("classifier"),
        "fragment" => Lifetime::Wrapped("fragment"),
        "todo_intercept" => Lifetime::Wrapped("todo"),
        "stream_retry" => Lifetime::Wrapped("stream_retry"),
        "todo_nudge" | "length_redrive" | "repeat_break" | "spend_alert" | "discovery"
        | "cache_alert" | "plan_relevance" | "todo_prelude" => Lifetime::OnceRead,
        _ => Lifetime::Durable,
    }
}

/// The details key a goal notice carries; a `goal_prompt` without it is a continuation, which
/// includes every one an older build recorded.
pub const NOTICE_DETAIL: &str = "goalNotice";

fn is_continuation(message: &AgentMessage) -> bool {
    matches!(message, AgentMessage::Custom { custom_type, details, .. }
        if custom_type == "goal_prompt"
            && details.as_ref().and_then(|details| details.get(NOTICE_DETAIL)).is_none())
}

/// Invariant: a request carries only the newest goal continuation, and no cut re-drive a reply
/// followed; both are scanned for once, so the test per message is a comparison.
pub fn spent_in_request(messages: &[AgentMessage]) -> impl Fn(usize, &AgentMessage) -> bool + '_ {
    let reply = messages
        .iter()
        .rposition(|message| matches!(message, AgentMessage::Assistant { .. }));
    let newest = messages.iter().rposition(is_continuation);
    move |index, message| match message {
        AgentMessage::Custom { custom_type, .. } if custom_type == "length_redrive" => {
            reply > Some(index)
        }
        _ => is_continuation(message) && newest != Some(index),
    }
}

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
                return match lifetime_of(custom_type) {
                    Lifetime::Wrapped(_) => false,
                    Lifetime::OnceRead => !read,
                    Lifetime::Durable => true,
                };
            }
            internal_source(message).is_none()
        })
        .map(|(_, message)| message.clone())
        .collect()
}
