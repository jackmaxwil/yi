use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Attribution, Content, UserContent};
use yi_types::plan::doc::TodoState;
use yi_types::todo::{Claim, TodoList};

use crate::AgentSession;

#[derive(Default)]
pub struct History {
    scanned_seq: u64,
    seen: String,
    written: Vec<(String, String)>,
    touched: Option<u64>,
    claims: Vec<Claim>,
}

impl History {
    fn observe_records(
        &mut self,
        store: &yi_session::SharedSession,
    ) -> (usize, Vec<(String, String)>) {
        let mut after = self.scanned_seq;
        let mut entries = Vec::new();
        yi_session::lock_session(store).find_on_branch("main", |entry| {
            if entry.seq() < after {
                *self = Self::default();
                after = 0;
            }
            if entry.seq() <= after {
                return Some(());
            }
            entries.push(entry.clone());
            None
        });
        let count = entries.len();
        entries.reverse();
        let mut records = Vec::new();
        for entry in entries {
            self.scanned_seq = self.scanned_seq.max(entry.seq());
            let Entry::Message { message, .. } = entry else {
                continue;
            };
            records.extend(
                records_of(&message)
                    .into_iter()
                    .map(|(id, text)| (id.to_owned(), text.to_owned())),
            );
            match message {
                AgentMessage::User {
                    content: UserContent::Text(text),
                    attribution,
                    ..
                } => {
                    if attribution == Attribution::User {
                        self.seen.clear();
                        self.written.clear();
                    }
                    self.seen.push_str(&text);
                    self.seen.push('\n');
                }
                AgentMessage::ToolResult { content, .. } => {
                    for block in content {
                        if let Content::Text { text, .. } = block {
                            self.seen.push_str(&text);
                            self.seen.push('\n');
                        }
                    }
                }
                AgentMessage::Assistant { content, .. } => {
                    for block in content {
                        let Content::ToolCall {
                            name, arguments, ..
                        } = block
                        else {
                            continue;
                        };
                        if name != "write" {
                            continue;
                        }
                        let path = arguments.get("path").and_then(serde_json::Value::as_str);
                        let body = arguments.get("content").and_then(serde_json::Value::as_str);
                        if let (Some(path), Some(body)) = (path, body)
                            && super::coupling::data_path(path)
                        {
                            self.written.push((path.to_owned(), body.to_owned()));
                        }
                    }
                }
                _ => {}
            }
        }
        (count, records)
    }

    pub fn observe(&mut self, store: &yi_session::SharedSession) -> usize {
        self.observe_records(store).0
    }

    pub fn claims_cached(
        &mut self,
        list: &TodoList,
        store: &yi_session::SharedSession,
        touched: u64,
    ) -> Vec<Claim> {
        if self.touched != Some(touched) {
            self.scanned_seq = 0;
            self.seen.clear();
            self.written.clear();
            self.claims.clear();
        }
        let previous = std::mem::take(&mut self.claims);
        let (_, records) = self.observe_records(store);
        if self.touched == Some(touched) && records.is_empty() {
            self.claims = previous;
            return self.claims.clone();
        }
        self.claims = claims_from_records(list, &records)
            .into_iter()
            .map(|mut claim| {
                if claim.observed.is_none() {
                    claim.observed = previous
                        .iter()
                        .find(|old| old.label == claim.label)
                        .and_then(|old| old.observed.clone());
                }
                claim
            })
            .collect();
        self.touched = Some(touched);
        self.claims.clone()
    }

    pub(crate) fn start_prompt(&mut self) {
        self.written.clear();
    }

    pub(crate) fn seen(&self) -> &str {
        &self.seen
    }

    pub(crate) fn written(&self) -> &[(String, String)] {
        &self.written
    }
}

pub fn claims(list: &TodoList, entries: &[Entry]) -> Vec<Claim> {
    let records: Vec<(String, String)> = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Message { message, .. } => Some(message),
            _ => None,
        })
        .flat_map(records_of)
        .map(|(id, text)| (id.to_owned(), text.to_owned()))
        .collect();
    claims_from_records(list, &records)
}

fn claims_from_records(list: &TodoList, records: &[(String, String)]) -> Vec<Claim> {
    list.items()
        .filter(|item| matches!(item.state, TodoState::Done { .. }))
        .map(|item| {
            let spans = quoted(item.evidence.as_deref().unwrap_or(""));
            let observed = records
                .iter()
                .find(|(_, text)| spans.iter().any(|span| text.contains(span)))
                .map(|(id, _)| id.clone());
            Claim {
                label: item.label.as_str().to_owned(),
                observed,
            }
        })
        .collect()
}

pub fn session_claims(session: &AgentSession) -> Vec<Claim> {
    let (Some(todos), Some(store)) = (session.todos(), session.store()) else {
        return Vec::new();
    };
    let Ok(mut history) = todos.history.lock() else {
        return Vec::new();
    };
    history.claims_cached(&todos.list(), &store, todos.touched())
}
fn quoted(evidence: &str) -> Vec<&str> {
    evidence
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::trim)
        .filter(|span| span.chars().count() >= 4)
        .collect()
}

/// Invariant: the list's own calls carry the evidence under test, so they can never observe it.
fn records_of(message: &AgentMessage) -> Vec<(&str, &str)> {
    let own = |name: &str| name == crate::todo::tool::NAME || name == "plan";
    match message {
        AgentMessage::Assistant { content, .. } => content
            .iter()
            .flat_map(|block| match block {
                Content::ToolCall {
                    id,
                    name,
                    arguments,
                    ..
                } if !own(name) => arguments
                    .values()
                    .filter_map(|value| value.as_str().map(|text| (id.as_str(), text)))
                    .collect(),
                _ => Vec::new(),
            })
            .collect(),
        AgentMessage::ToolResult {
            tool_call_id,
            tool_name,
            content,
            ..
        } if !own(tool_name) => content
            .iter()
            .filter_map(|block| match block {
                Content::Text { text, .. } => Some((tool_call_id.as_str(), text.as_str())),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seen_text_covers_only_the_current_typed_prompt() -> Result<(), Box<dyn std::error::Error>> {
        let store = std::sync::Arc::new(std::sync::Mutex::new(
            yi_session::SessionStore::in_memory(yi_session::SessionMetadata {
                id: "bounded-claims".to_owned(),
                created_at: 0,
                parent_session_id: None,
                name: None,
            }),
        ));
        for (seq, text) in [(1, "old prompt"), (2, "current prompt")] {
            let entry = Entry::Message {
                id: format!("e{seq}"),
                message: serde_json::from_value(serde_json::json!({
                    "role": "user", "content": text, "attribution": "user", "timestamp": seq
                }))?,
                terminate: None,
                parent_id: None,
                seq,
                timestamp: seq,
            };
            yi_session::lock_session(&store).append_entry(entry, "main")?;
        }
        let mut history = History::default();
        assert_eq!(history.observe(&store), 2);
        assert_eq!(history.seen(), "current prompt\n");
        Ok(())
    }
}
