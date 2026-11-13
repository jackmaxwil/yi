use std::sync::Arc;

use yi_types::message::{AgentMessage, UserContent};

use super::{Shared, extensions_of};
use crate::ext::Host;

pub(super) fn assembled_prompt(
    shared: &Arc<Shared>,
    fallback: &str,
    read: fn(&Host) -> String,
) -> String {
    let Some(host) = extensions_of(shared) else {
        return fallback.to_owned();
    };
    let assembled = host.lock().map(|host| read(&host)).unwrap_or_default();
    if assembled.is_empty() {
        fallback.to_owned()
    } else {
        assembled
    }
}

pub(super) fn prompt_text(prompt: &AgentMessage) -> String {
    match prompt {
        AgentMessage::User {
            content: UserContent::Text(text),
            ..
        } => text.clone(),
        AgentMessage::User {
            content: UserContent::Blocks(blocks),
            ..
        } => yi_types::message::join_text(blocks, "\n"),
        _ => String::new(),
    }
}
