use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content, UserContent};

use crate::session::AgentSession;

pub struct Rewound {
    pub leaf: Option<String>,
    /// Handed back for the composer: landing on a user turn means editing it.
    pub unsent: Option<String>,
}

/// Selecting a user message rewinds to its *parent*: the point of picking your
/// own message is to stand where you were before you sent it.
pub fn rewind_to(session: &AgentSession, entry_id: &str) -> Result<Rewound, String> {
    let store = session.store().ok_or("no session store")?;
    let entry = yi_session::lock_session(&store)
        .entry(entry_id)
        .ok_or_else(|| format!("no entry {entry_id}"))?;
    let (leaf, unsent) = match &entry {
        Entry::Message {
            message: AgentMessage::User { content, .. },
            parent_id,
            ..
        } => (parent_id.clone(), Some(user_text(content))),
        Entry::Custom {
            custom_type,
            data,
            parent_id,
            ..
        } => (
            parent_id.clone(),
            custom_text(custom_type, data.as_ref()).into(),
        ),
        other => (Some(other.id().to_owned()), None),
    };
    yi_session::lock_session(&store)
        .move_lane("main", leaf.as_deref())
        .map_err(|error| error.to_string())?;
    session
        .attach_store(store)
        .map_err(|error| error.to_string())?;
    Ok(Rewound { leaf, unsent })
}

fn user_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn custom_text(custom_type: &str, data: Option<&serde_json::Value>) -> String {
    data.and_then(|data| data.get("text"))
        .and_then(serde_json::Value::as_str)
        .map_or_else(|| format!("[{custom_type}]"), str::to_owned)
}
