use std::time::Duration;

use yi_types::entry::Entry;
use yi_types::message::AgentMessage;
use yi_types::wire::{JsonlV4Header, Mutation};

/// A bare assistant message per line replays at once; a recorded session's assistant entries
/// replay over the time each took after the message that prompted it (user or tool result).
pub fn cassette(source: &str) -> Result<Vec<(AgentMessage, Duration)>, String> {
    let mut replies = Vec::new();
    let mut prompted_at = 0_u64;
    for (index, line) in source.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Mutation>(line) {
            Ok(Mutation::Entry {
                entry: Entry::Message {
                    message, timestamp, ..
                },
                ..
            }) => {
                if matches!(message, AgentMessage::Assistant { .. }) {
                    let lag = match prompted_at {
                        0 => 0,
                        from => timestamp.saturating_sub(from),
                    };
                    replies.push((message, Duration::from_millis(lag)));
                } else {
                    prompted_at = timestamp;
                }
            }
            Ok(_) => {}
            Err(_) if serde_json::from_str::<JsonlV4Header>(line).is_ok() => {}
            Err(_) => {
                let message = serde_json::from_str(line)
                    .map_err(|error| format!("line {}: {error}", index + 1))?;
                replies.push((message, Duration::ZERO));
            }
        }
    }
    Ok(replies)
}
