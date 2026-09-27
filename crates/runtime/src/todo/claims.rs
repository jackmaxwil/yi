use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Content};
use yi_types::plan::doc::TodoStateName;
use yi_types::todo::{Claim, TodoList};

use crate::AgentSession;

pub fn claims(list: &TodoList, entries: &[Entry]) -> Vec<Claim> {
    let records: Vec<(&str, &str)> = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Message { message, .. } => Some(message),
            _ => None,
        })
        .flat_map(records_of)
        .collect();
    list.items()
        .filter(|item| item.state == TodoStateName::Done)
        .filter_map(|item| {
            let spans = quoted(item.evidence.as_deref()?);
            let observed = records
                .iter()
                .find(|(_, text)| spans.iter().any(|span| text.contains(span)))
                .map(|(id, _)| (*id).to_owned());
            Some(Claim {
                label: item.label.as_str().to_owned(),
                observed,
            })
        })
        .collect()
}

pub fn session_claims(session: &AgentSession) -> Vec<Claim> {
    let (Some(todos), Some(store)) = (session.todos(), session.store()) else {
        return Vec::new();
    };
    let entries = yi_session::lock_session(&store)
        .find_entries_on_branch(
            "main",
            &yi_session::EntryQuery {
                order: yi_session::EntryOrder::OldestFirst,
                ..yi_session::EntryQuery::default()
            },
            &yi_session::BranchBounds::default(),
        )
        .unwrap_or_default();
    claims(&todos.list(), &entries)
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

fn records_of(message: &AgentMessage) -> Vec<(&str, &str)> {
    match message {
        AgentMessage::Assistant { content, .. } => content
            .iter()
            .flat_map(|block| match block {
                Content::ToolCall { id, arguments, .. } => arguments
                    .values()
                    .filter_map(|value| value.as_str().map(|text| (id.as_str(), text)))
                    .collect(),
                _ => Vec::new(),
            })
            .collect(),
        AgentMessage::ToolResult {
            tool_call_id,
            content,
            ..
        } => content
            .iter()
            .filter_map(|block| match block {
                Content::Text { text, .. } => Some((tool_call_id.as_str(), text.as_str())),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}
