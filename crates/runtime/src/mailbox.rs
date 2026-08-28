use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_types::message::AgentMessage;

use crate::subagent::{ChildStatus, PARENT_NAME, SubagentHost, last_assistant_text};

pub(crate) const WAIT_MIN_MS: u64 = 1_000;
pub(crate) const WAIT_MAX_MS: u64 = 300_000;
const WAIT_POLL_MS: u64 = 100;

/// Invariant: an agent's words reach another agent inside this envelope and
/// never as bare user text — provenance the model can see (B6 hardening).
fn envelope(from: &str, text: &str) -> String {
    format!("<agent_message from=\"{from}\">\n{text}\n</agent_message>")
}

fn agent_message(from: &str, text: &str) -> AgentMessage {
    AgentMessage::Custom {
        custom_type: "agent_message".to_owned(),
        content: yi_types::message::UserContent::Text(envelope(from, text)),
        display: true,
        details: None,
        timestamp: yi_session::now_ms(),
    }
}

fn receipt(target: &str, state: &str) -> Value {
    json!({"target": target, "state": state})
}

pub(crate) fn message_params(payload: &Map<String, Value>) -> (String, Option<String>, bool) {
    let target = payload
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let text = payload
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let followup = payload
        .get("followup")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    (target, text, followup)
}

/// The child half of B6: a name is looked for locally, then in the family.
pub fn register_child_messaging(
    link: ParentLink,
    local: &Arc<SubagentHost>,
    registry: &mut crate::kernel::HostRegistry,
) {
    let sender = link.clone();
    let own = Arc::clone(local);
    registry.register("agent_message.send", move |payload| {
        let (target, text, followup) = message_params(&payload);
        let sender = sender.clone();
        let own = Arc::clone(&own);
        Box::pin(async move {
            let text = text.ok_or("agent_message.send requires a message")?;
            if matches!(target.as_str(), "parent" | "all") {
                return sender.send(&target, &text, followup);
            }
            own.route(PARENT_NAME, &target, &text, followup)
                .or_else(|_| sender.send(&target, &text, followup))
        })
    });
    registry.register("agent_message.list_agents", move |_payload| {
        let reply = link.roster();
        Box::pin(async move { Ok(reply) })
    });
}

/// What a child needs to reach the family it was spawned into: its own name,
/// and the parent host's router (a `Weak`, so the family is not a cycle).
#[derive(Clone)]
pub struct ParentLink {
    pub child_name: String,
    pub host: std::sync::Weak<SubagentHost>,
}

impl ParentLink {
    pub fn send(
        &self,
        target: &str,
        text: &str,
        followup: bool,
    ) -> Result<Map<String, Value>, String> {
        let host = self
            .host
            .upgrade()
            .ok_or_else(|| "the parent session is gone".to_owned())?;
        host.route(&self.child_name, target, text, followup)
    }

    pub fn roster(&self) -> Map<String, Value> {
        self.host
            .upgrade()
            .map(|host| host.roster())
            .unwrap_or_default()
    }
}

impl SubagentHost {
    /// B6 routing, one seam for all three directions: a child reaches its
    /// parent or a named sibling, the parent reaches one child or `all`.
    pub fn route(
        &self,
        from: &str,
        target: &str,
        text: &str,
        followup: bool,
    ) -> Result<Map<String, Value>, String> {
        if target.trim().is_empty() {
            return Err(
                "agent_message.send needs a target (a name, \"parent\", or \"all\")".to_owned(),
            );
        }
        if target == from {
            return Err(format!("agent \"{from}\" cannot send to itself"));
        }
        match target {
            "parent" => {
                if from == PARENT_NAME {
                    return Err("the parent has no parent to message".to_owned());
                }
                self.deliver_to_parent(from, text);
                let mut reply = Map::new();
                reply.insert(
                    "receipts".to_owned(),
                    json!([receipt("parent", "delivered")]),
                );
                Ok(reply)
            }
            "all" => {
                let names: Vec<String> = self
                    .children
                    .lock()
                    .map(|children| {
                        children
                            .values()
                            .map(|record| record.session_name.clone())
                            .filter(|name| name != from)
                            .collect()
                    })
                    .unwrap_or_default();
                // allSettled: one unreachable target never voids the fan-out.
                let receipts: Vec<Value> = names
                    .iter()
                    .map(
                        |name| match self.deliver_to_child(from, name, text, followup) {
                            Ok(state) => receipt(name, state),
                            Err(error) => receipt(name, &error),
                        },
                    )
                    .collect();
                let mut reply = Map::new();
                reply.insert("receipts".to_owned(), Value::Array(receipts));
                Ok(reply)
            }
            name => {
                let state = self.deliver_to_child(from, name, text, followup)?;
                let mut reply = Map::new();
                reply.insert("receipts".to_owned(), json!([receipt(name, state)]));
                Ok(reply)
            }
        }
    }

    fn deliver_to_parent(&self, from: &str, text: &str) {
        if let Ok(mut children) = self.children.lock()
            && let Ok(key) = Self::key_of(&children, from)
            && let Some(record) = children.get_mut(&key)
        {
            record.pending = record.pending.saturating_add(1);
        }
        (self.options.report)(agent_message(from, text));
    }

    fn deliver_to_child(
        &self,
        from: &str,
        target: &str,
        text: &str,
        followup: bool,
    ) -> Result<&'static str, String> {
        let children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned".to_owned())?;
        let key = Self::key_of(&children, target).map_err(|_| {
            let known: Vec<&str> = children
                .values()
                .map(|record| record.session_name.as_str())
                .collect();
            format!(
                "no agent named \"{target}\"; known children: {}",
                if known.is_empty() {
                    "(none)".to_owned()
                } else {
                    known.join(", ")
                }
            )
        })?;
        let record = children
            .get(&key)
            .ok_or_else(|| format!("no agent named \"{target}\""))?;
        let message = agent_message(from, text);
        if followup {
            record.session.deliver(message);
            Ok("delivered")
        } else {
            record.session.follow_up_message(message);
            Ok("queued")
        }
    }

    pub fn roster(&self) -> Map<String, Value> {
        let mut agents = vec![json!({"name": PARENT_NAME, "role": "parent"})];
        if let Ok(children) = self.children.lock() {
            let mut names: Vec<(&str, &'static str)> = children
                .values()
                .map(|record| (record.session_name.as_str(), record.status.as_str()))
                .collect();
            names.sort_unstable();
            agents.extend(
                names
                    .into_iter()
                    .map(|(name, status)| json!({"name": name, "role": "child", "status": status})),
            );
        }
        let mut reply = Map::new();
        reply.insert("agents".to_owned(), Value::Array(agents));
        reply
    }

    /// B13 wait: which children moved since the last call; the clamp is reported.
    pub async fn wait(&self, timeout_ms: u64) -> Map<String, Value> {
        let clamped = timeout_ms.clamp(WAIT_MIN_MS, WAIT_MAX_MS);
        let deadline = std::time::Instant::now()
            .checked_add(std::time::Duration::from_millis(clamped))
            .unwrap_or_else(std::time::Instant::now);
        loop {
            let updated = self.take_pending();
            if !updated.is_empty() || std::time::Instant::now() >= deadline {
                let mut reply = Map::new();
                reply.insert("updated".to_owned(), json!(updated));
                reply.insert("timeout_ms".to_owned(), Value::from(clamped));
                reply.insert("clamped".to_owned(), Value::Bool(clamped != timeout_ms));
                return reply;
            }
            tokio::time::sleep(std::time::Duration::from_millis(WAIT_POLL_MS)).await;
        }
    }

    fn take_pending(&self) -> Vec<String> {
        let Ok(mut children) = self.children.lock() else {
            return Vec::new();
        };
        let mut moved: Vec<String> = children
            .values_mut()
            .filter(|record| record.pending > 0)
            .map(|record| {
                record.pending = 0;
                record.session_name.clone()
            })
            .collect();
        moved.sort();
        moved
    }

    /// B13 interrupt: ends the run and keeps the record, unlike delete.
    pub fn interrupt(&self, target: &str) -> Result<Map<String, Value>, String> {
        let children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned")?;
        let key = Self::key_of(&children, target)?;
        let record = children
            .get(&key)
            .ok_or_else(|| format!("No RLM child matches \"{target}\""))?;
        record.session.abort();
        let mut reply = Map::new();
        reply.insert("interrupted".to_owned(), Value::String(key));
        Ok(reply)
    }

    /// The child's answer as data in the parent's kernel namespace: JSON when
    /// it parses, checked against `schema` at this seam when one is given.
    pub fn result(
        &self,
        target: &str,
        schema: Option<&Value>,
    ) -> Result<Map<String, Value>, String> {
        let children = self
            .children
            .lock()
            .map_err(|_| "subagent state poisoned")?;
        let key = Self::key_of(&children, target)?;
        let record = children
            .get(&key)
            .ok_or_else(|| format!("No RLM child matches \"{target}\""))?;
        if record.status == ChildStatus::Running {
            return Err(format!("child \"{target}\" is still running"));
        }
        if let Some(error) = &record.error {
            return Err(format!("child \"{target}\" failed: {error}"));
        }
        let text = last_assistant_text(&record.session.messages()).unwrap_or_default();
        let json = serde_json::from_str::<Value>(text.trim()).ok();
        if let Some(schema) = schema {
            let value = json
                .clone()
                .ok_or_else(|| format!("child \"{target}\" did not answer with JSON:\n{text}"))?;
            crate::schema::Schema::from_value(schema.clone())
                .validate(&value)
                .map_err(|error| format!("child \"{target}\" result rejected: {error}"))?;
        }
        let mut reply = Map::new();
        reply.insert(
            "name".to_owned(),
            Value::String(record.session_name.clone()),
        );
        reply.insert("text".to_owned(), Value::String(text));
        if let Some(json) = json {
            reply.insert("json".to_owned(), json);
        }
        Ok(reply)
    }
}
