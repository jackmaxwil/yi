//! `_yi/fork`: a new session branched from one of this session's messages, attached beside it.
use serde_json::{Value, json};
use yi_runtime::session_store::{
    CreateOptions, ForkPosition, ForkScope, SessionRepo, lock_session,
};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content, UserContent};

use crate::{AcpState, INVALID_PARAMS};

fn typed(message: &AgentMessage) -> String {
    match message {
        AgentMessage::User {
            content: UserContent::Text(text),
            ..
        } => text.clone(),
        AgentMessage::User {
            content: UserContent::Blocks(blocks),
            ..
        } => blocks
            .iter()
            .filter_map(|block| match block {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

impl AcpState {
    /// `before` leaves the entry out and hands back what the person typed in it, to ask again;
    /// `at` keeps it, so a side question starts from that answer. The source session is untouched.
    pub(crate) fn fork(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let (handle, session_id) = self.session(params)?;
        let entry_id = params
            .get("entryId")
            .and_then(Value::as_str)
            .ok_or((INVALID_PARAMS, "missing entryId".to_owned()))?
            .to_owned();
        let position = match params.get("position").and_then(Value::as_str) {
            Some("at") => ForkPosition::At,
            Some("before") | None => ForkPosition::Before,
            Some(other) => {
                return Err((
                    INVALID_PARAMS,
                    format!("position is before or at, not {other}"),
                ));
            }
        };
        let prefill = match (&position, handle.session.store()) {
            (ForkPosition::Before, Some(store)) => match lock_session(&store).entry(&entry_id) {
                Some(Entry::Message { message, .. }) => typed(&message),
                _ => String::new(),
            },
            _ => String::new(),
        };
        let scope = ForkScope::Branch {
            entry_id: Some(entry_id),
            position: Some(position),
        };
        let store = self
            .repo
            .fork(&session_id, &scope, CreateOptions::default())
            .map_err(|error| (INVALID_PARAMS, error.to_string()))?;
        let forked = self.attach_later(&store);
        let mut result = json!(self.session_result(&forked));
        result["configOptions"] = json!(self.default_options());
        result["prefill"] = Value::String(prefill);
        Ok(result)
    }
}
