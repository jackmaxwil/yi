//! The worker's event forwarders: the runtime's events verbatim as `_yi/event`, the
//! standard ACP projection beside them, and the child streams the parent adopts.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use tokio::task::JoinSet;
use yi_runtime::session_store::{EntryOrder, EntryQuery, SharedSession, lock_session};
use yi_runtime::{AgentSession, SubagentHost};
use yi_types::acp::AcpSessionUpdate;
use yi_types::entry::Entry;
use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, Content, UserContent};
use yi_types::subagent::ChildId;

use crate::update::{IdMap, event_update, extension, gap_update, to_updates};
use crate::{LineSink, StatusLedger, status_update, update_notification};

pub(crate) struct Forward {
    pub(crate) session_id: String,
    pub(crate) child: Option<ChildId>,
    pub(crate) seq: Arc<AtomicU64>,
    pub(crate) sink: LineSink,
}

impl Forward {
    fn emit(&self, update: AcpSessionUpdate) {
        (self.sink)(&update_notification(&self.session_id, update));
    }

    fn event(&self, event: &AgentEvent) {
        let seq = match self
            .seq
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        {
            Ok(seq) => seq,
            Err(_) => {
                self.seq.store(0, Ordering::Relaxed);
                self.emit(gap_update(0, 1, self.child.as_ref()));
                0
            }
        };
        self.emit(event_update(event, seq, self.child.as_ref()));
    }

    fn gap(&self, dropped: u64) {
        let seq = self.seq.load(Ordering::Relaxed);
        self.emit(gap_update(seq, dropped, self.child.as_ref()));
    }
}

async fn forward_child(mut events: tokio::sync::broadcast::Receiver<AgentEvent>, forward: Forward) {
    loop {
        match events.recv().await {
            Ok(event) => forward.event(&event),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(dropped)) => forward.gap(dropped),
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

pub(crate) struct Parent {
    pub(crate) forward: Forward,
    pub(crate) session: Arc<AgentSession>,
    pub(crate) host: Arc<SubagentHost>,
    pub(crate) ids: IdMap,
    pub(crate) ledger: Arc<std::sync::Mutex<StatusLedger>>,
    pub(crate) context_window: u64,
    pub(crate) children: JoinSet<()>,
    pub(crate) seen: HashSet<String>,
    pub(crate) last_goal: Value,
}

impl Parent {
    fn adopt_children(&mut self) {
        for child in self.host.children_view() {
            if !self.seen.insert(child.update.id.0.clone()) {
                continue;
            }
            let forward = Forward {
                session_id: self.forward.session_id.clone(),
                child: Some(child.update.id.clone()),
                seq: Arc::clone(&self.forward.seq),
                sink: Arc::clone(&self.forward.sink),
            };
            self.children
                .spawn(forward_child(child.session.subscribe(), forward));
        }
    }

    fn watch_goal(&mut self) {
        let goal = self
            .session
            .store()
            .and_then(|store| lock_session(&store).goal())
            .and_then(|goal| serde_json::to_value(goal).ok())
            .unwrap_or(Value::Null);
        if goal != self.last_goal {
            self.last_goal = goal.clone();
            self.forward.emit(extension("_yi/goal", [("goal", goal)]));
        }
    }

    fn account(&mut self, event: &AgentEvent) {
        let AgentEvent::MessageEnd {
            message: AgentMessage::Assistant { usage, .. },
        } = event
        else {
            return;
        };
        let snapshot = self.ledger.lock().map(|mut ledger| {
            ledger.cost += usage.cost.total.as_f64().unwrap_or(0.0);
            ledger.cost_unknown |= usage.unknown;
            ledger.context_used = u64::try_from(usage.total_tokens).unwrap_or(0);
            *ledger
        });
        if let Ok(snapshot) = snapshot {
            self.forward
                .emit(status_update(None, snapshot, self.context_window));
        }
    }

    fn reduce(&mut self, event: &AgentEvent) {
        self.forward.event(event);
        for update in to_updates(event, &mut self.ids) {
            self.forward.emit(update);
        }
        self.account(event);
        match event {
            AgentEvent::ChildUpdate { .. } => self.adopt_children(),
            AgentEvent::MessageEnd { .. }
            | AgentEvent::ToolExecutionEnd { .. }
            | AgentEvent::AgentEnd { .. } => self.watch_goal(),
            _ => {}
        }
    }
}

pub(crate) async fn forward_parent(
    mut events: tokio::sync::broadcast::Receiver<AgentEvent>,
    mut parent: Parent,
) {
    loop {
        match events.recv().await {
            Ok(event) => parent.reduce(&event),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(dropped)) => {
                parent.forward.gap(dropped);
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

pub(crate) fn session_name(store: &SharedSession) -> Option<String> {
    let session = lock_session(store);
    if let Some(name) = session.name() {
        return Some(name);
    }
    let entries = session
        .find_entries(&EntryQuery {
            order: EntryOrder::OldestFirst,
            ..EntryQuery::default()
        })
        .ok()?;
    entries.iter().find_map(|entry| match entry {
        Entry::Message {
            message: AgentMessage::User { content, .. },
            ..
        } => yi_runtime::session_store::session_title(&prompt_of(content)),
        _ => None,
    })
}

fn prompt_of(content: &UserContent) -> String {
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
