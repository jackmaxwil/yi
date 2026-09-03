use std::sync::Arc;
use std::sync::mpsc::Sender;

use serde_json::Value;
use yi_runtime::{AgentSession, SubagentHost};
use yi_types::entry::Entry;
use yi_types::message::AgentMessage;

use super::{App, UiEvent};
use crate::cell::{Cell, ToolCell, ToolStatus};
use crate::transcript::{text_of, user_text};

pub(crate) fn sync_roster(
    app: &mut App,
    host: &Arc<SubagentHost>,
    handle: &tokio::runtime::Handle,
    ui_tx: &Sender<UiEvent>,
) {
    let children = host.children_view();
    for child in &children {
        let subscribed = app
            .tasks
            .get(child.update.id.as_str())
            .is_some_and(|state| state.subscribed);
        if !subscribed {
            let mut events = child.session.subscribe();
            let child_id = child.update.id.as_str().to_owned();
            let ui_tx = ui_tx.clone();
            handle.spawn(async move {
                while let Ok(event) = events.recv().await {
                    if ui_tx
                        .send(UiEvent::Child {
                            child_id: child_id.clone(),
                            event,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }
    }
    app.sync_children(&children);
}

/// The active branch only — [`entries_of`] returns the whole tree for the tree
/// view, and a transcript built from that leaves rewound turns on screen.
fn branch_of(session: &AgentSession) -> Vec<Entry> {
    let Some(store) = session.store() else {
        return Vec::new();
    };
    yi_runtime::session_store::lock_session(&store)
        .find_entries_on_branch(
            "main",
            &yi_runtime::session_store::EntryQuery {
                order: yi_runtime::session_store::EntryOrder::OldestFirst,
                ..yi_runtime::session_store::EntryQuery::default()
            },
            &yi_runtime::session_store::BranchBounds::default(),
        )
        .unwrap_or_default()
}

pub(crate) fn entries_of(session: &AgentSession) -> (Vec<Entry>, Option<String>) {
    let Some(store) = session.store() else {
        return (Vec::new(), None);
    };
    let locked = yi_runtime::session_store::lock_session(&store);
    let entries = locked
        .find_entries(&yi_runtime::session_store::EntryQuery {
            order: yi_runtime::session_store::EntryOrder::OldestFirst,
            ..yi_runtime::session_store::EntryQuery::default()
        })
        .unwrap_or_default();
    let leaf = locked.leaf_id("main").ok().flatten();
    (entries, leaf)
}

pub(crate) fn replay_child(app: &mut App, child_id: &str) {
    let Some(session) = app.tasks.get(child_id).map(|s| Arc::clone(&s.session)) else {
        return;
    };
    replay_session(app, &session);
}

/// The model's context and the screen must agree about what was said.
pub(crate) fn replay_session(app: &mut App, session: &AgentSession) {
    let entries = branch_of(session);
    let cells: Vec<Cell> = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Message { message, .. } => match message {
                AgentMessage::User { content, .. } => Some(Cell::User {
                    text: user_text(content),
                }),
                AgentMessage::Assistant { content, .. } => {
                    let text = text_of(content);
                    if text.is_empty() {
                        None
                    } else {
                        Some(Cell::Assistant { markdown: text })
                    }
                }
                AgentMessage::ToolResult {
                    tool_name,
                    content,
                    is_error,
                    details,
                    ..
                } => Some(Cell::Tool(ToolCell {
                    name: tool_name.clone(),
                    // A replayed entry has no live call to pair with, and the
                    // session never recorded how long the call took.
                    call_id: String::new(),
                    intent: None,
                    status: if *is_error {
                        ToolStatus::Failed
                    } else {
                        ToolStatus::Done
                    },
                    summary: ToolCell::summary_of(tool_name, ""),
                    digest: ToolCell::digest_of(tool_name, &text_of(content), *is_error),
                    preview: Vec::new(),
                    elapsed_ms: 0,
                    calls: 1,
                    details: details.clone().unwrap_or(Value::Null),
                })),
                _ => None,
            },
            _ => None,
        })
        .collect();
    for cell in cells {
        app.commit_cell(&cell);
    }
}
