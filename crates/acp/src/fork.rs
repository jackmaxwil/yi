//! `_yi/fork`: a new session branched from one of this session's messages, attached beside it.
use serde_json::{Value, json};
use yi_runtime::session_store::{
    CreateOptions, ForkPosition, ForkScope, SessionRepo, lock_session,
};
use yi_types::acp::{AcpForkParams, AcpForkPosition, AcpMeta};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, UserContent};

use crate::{AcpState, INVALID_PARAMS};

/// The prompt as Rewind hands it back, so Re-ask and Rewind put the same text in the composer.
fn typed(message: &AgentMessage) -> String {
    match message {
        AgentMessage::User {
            content: UserContent::Text(text),
            ..
        } => text.clone(),
        AgentMessage::User {
            content: UserContent::Blocks(blocks),
            ..
        } => yi_types::message::join_text(blocks, " "),
        _ => String::new(),
    }
}

impl AcpState {
    /// `before` leaves the entry out and hands back what the person typed in it, to ask again;
    /// `at` keeps it, so a side question starts from that answer. The source session is untouched.
    pub(crate) fn fork(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let (handle, session_id) = self.session(params)?;
        let asked: AcpForkParams = serde_json::from_value(params.clone())
            .map_err(|error| (INVALID_PARAMS, error.to_string()))?;
        let position = match asked.position {
            AcpForkPosition::Before => ForkPosition::Before,
            AcpForkPosition::At => ForkPosition::At,
            AcpForkPosition::Other(other) => {
                return Err((
                    INVALID_PARAMS,
                    format!("position is before or at, not {other}"),
                ));
            }
        };
        let prefill = match (&position, handle.session.store()) {
            (ForkPosition::Before, Some(store)) => {
                match lock_session(&store).entry(&asked.entry_id) {
                    Some(Entry::Message { message, .. }) => Some(typed(&message)),
                    _ => None,
                }
            }
            _ => None,
        };
        let scope = ForkScope::Branch {
            entry_id: Some(asked.entry_id),
            position: Some(position),
        };
        let store = self
            .repo
            .fork(&session_id, &scope, CreateOptions::default())
            .map_err(|error| (INVALID_PARAMS, error.to_string()))?;
        let forked = self.attach_later(&store);
        let mut result = self.session_result(&forked);
        result.meta.get_or_insert_with(AcpMeta::default).yi.prefill = prefill;
        Ok(json!(result))
    }
}
