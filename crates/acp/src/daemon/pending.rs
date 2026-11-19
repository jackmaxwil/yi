//! The permission requests a worker has sent and no client has answered yet: listed by
//! `_yi/pending`, and sent again to a client that attaches after the first delivery.

use std::collections::HashSet;

use serde_json::{Value, json};
use yi_types::acp::{AcpPendingRequest, AcpPendingResult, AcpPermissionParams};

use super::{ClientId, Supervisor, now_ms};

pub(super) struct Held {
    pub(super) root: String,
    frame: Value,
    at_ms: u64,
    sent: HashSet<ClientId>,
}

impl Held {
    pub(super) fn new(root: String, frame: &Value) -> Self {
        Self {
            root,
            frame: frame.clone(),
            at_ms: now_ms(),
            sent: HashSet::new(),
        }
    }

    fn session(&self) -> Option<&str> {
        self.frame.pointer("/params/sessionId")?.as_str()
    }

    fn listed(&self) -> Option<AcpPendingRequest> {
        let params: AcpPermissionParams =
            serde_json::from_value(self.frame.get("params")?.clone()).ok()?;
        Some(AcpPendingRequest {
            request_id: self.frame.get("id")?.as_str()?.to_owned(),
            session_id: params.session_id,
            title: params.title,
            description: params.description,
            options: params.options,
            subject: params.subject,
            at_ms: Some(self.at_ms),
            meta: params.meta,
        })
    }
}

impl Supervisor {
    /// Remembers a worker's request until a client answers it or the worker closes it.
    pub(super) fn hold_request(&mut self, root: &str, method: &str, frame: &Value) {
        let id = frame.get("id").and_then(Value::as_str);
        if method == "_yi/permission_closed" {
            let closed = frame.pointer("/params/requestId").and_then(Value::as_str);
            self.worker_requests.remove(closed.unwrap_or_default());
        } else if let Some(id) = id {
            self.worker_requests
                .insert(id.to_owned(), Held::new(root.to_owned(), frame));
        }
    }

    pub(super) fn mark_sent(&mut self, request_id: Option<&str>, clients: &[ClientId]) {
        if let Some(held) = request_id.and_then(|id| self.worker_requests.get_mut(id)) {
            held.sent.extend(clients);
        }
    }

    pub(super) fn pending(&self, client: ClientId, id: Option<Value>, frame: &Value) {
        let Some(id) = id else { return };
        let session = frame.pointer("/params/sessionId").and_then(Value::as_str);
        let attached: HashSet<&str> = self
            .sessions
            .iter()
            .filter(|(_, entry)| entry.attached.contains(&client))
            .map(|(session, _)| session.as_str())
            .collect();
        let mut held: Vec<&Held> = self
            .worker_requests
            .values()
            .filter(|held| {
                held.session().is_some_and(|held_session| {
                    attached.contains(held_session)
                        && session.is_none_or(|session| held_session == session)
                })
            })
            .collect();
        held.sort_by_key(|held| held.at_ms);
        let requests = held.into_iter().filter_map(Held::listed).collect();
        let result = json!(AcpPendingResult { requests });
        self.send_client(client, super::result_frame(id, result));
    }

    pub(super) fn flush_parked(&mut self, session_id: &str, client: ClientId) {
        if let Some(entry) = self.sessions.get_mut(session_id) {
            (entry.headless, entry.owed) = (false, Vec::new());
        }
        for frame in self.parked.remove(session_id).unwrap_or_default() {
            self.send_client(client, frame.to_string());
        }
        let mut held: Vec<&mut Held> = self
            .worker_requests
            .values_mut()
            .filter(|held| held.session() == Some(session_id))
            .collect();
        held.sort_by_key(|held| held.at_ms);
        let lines: Vec<String> = held
            .into_iter()
            .filter_map(|held| held.sent.insert(client).then(|| held.frame.to_string()))
            .collect();
        for line in lines {
            self.send_client(client, line);
        }
    }
}
