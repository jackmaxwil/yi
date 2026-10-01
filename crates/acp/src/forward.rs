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
use yi_types::message::{AgentMessage, UserContent};
use yi_types::subagent::ChildId;

use crate::LineSink;
use crate::update::update_notification;
use crate::update::{IdMap, event_update, extension, gap_update, to_updates};

#[derive(Clone)]
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
    while let Some(next) = yi_runtime::next_event(&mut events).await {
        match next {
            Ok(event) => forward.event(&event),
            Err(gap) => forward.gap(gap.dropped),
        }
    }
}

pub(crate) struct Parent {
    pub(crate) forward: Forward,
    pub(crate) session: Arc<AgentSession>,
    pub(crate) host: Arc<SubagentHost>,
    pub(crate) ids: IdMap,
    pub(crate) children: JoinSet<()>,
    pub(crate) seen: HashSet<String>,
    pub(crate) last_goal: Value,
    pub(crate) last_workdir: Value,
    pub(crate) last_claims: Value,
    pub(crate) last_plan: Value,
    pub(crate) titled: bool,
    pub(crate) launch_cwd: std::path::PathBuf,
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

    pub(crate) fn watch_claims(&mut self) {
        let claims = serde_json::to_value(yi_runtime::todo::claims::session_claims(&self.session))
            .unwrap_or(Value::Null);
        if claims != self.last_claims {
            self.last_claims = claims.clone();
            self.forward
                .emit(extension("_yi/claims", [("claims", claims)]));
        }
    }

    pub(crate) fn watch_plan(&mut self) {
        let plan = self
            .session
            .plan_service()
            .and_then(|service| service.read_plan().ok())
            .map(|plan| {
                let progress = yi_types::plan::doc::progress(&plan.todos);
                serde_json::json!({
                    "done": progress.done,
                    "total": progress.total,
                    "running": progress.running.map(|label| label.to_string()),
                })
            })
            .unwrap_or(Value::Null);
        if plan != self.last_plan {
            self.last_plan = plan.clone();
            self.forward
                .emit(extension("_yi/plan_progress", [("plan", plan)]));
        }
    }

    fn title(&mut self) {
        let scripted = self.session.provider_arc().forces_faux()
            || self.session.summarizer().provider == yi_runtime::faux::FAUX_PROVIDER;
        if std::mem::replace(&mut self.titled, true) || scripted {
            return;
        }
        let (session, forward) = (Arc::clone(&self.session), self.forward.clone());
        tokio::spawn(async move {
            let update = match yi_runtime::title::title_session(&session).await {
                Ok(Some(name)) => extension("_yi/name", [("name", Value::String(name))]),
                Ok(None) => return,
                Err(error) => extension(
                    "_yi/notice",
                    [("text", Value::String(format!("session title: {error}")))],
                ),
            };
            forward.emit(update);
        });
    }

    pub(crate) fn watch_workdir(&mut self) {
        // A released lane hands the session back: say so, or the row names a lane that is gone.
        let workdir = match self.session.lane().and_then(|lane| lane.row()) {
            Some((path, row)) => {
                serde_json::json!({ "cwd": path.to_string_lossy(), "lane": row })
            }
            None => serde_json::json!({
                "cwd": self.launch_cwd.to_string_lossy(),
                "lane": Value::Null,
            }),
        };
        if workdir != self.last_workdir {
            self.last_workdir = workdir.clone();
            let fields = workdir.as_object().cloned().unwrap_or_default();
            self.forward.emit(extension("_yi/workdir", fields));
        }
    }

    fn reduce(&mut self, event: &AgentEvent) {
        self.forward.event(event);
        for update in to_updates(event, &mut self.ids) {
            self.forward.emit(update);
        }
        for prompt in self.ids.take_inserted() {
            let inserted = yi_types::acp::AcpPromptResult {
                message_id: prompt.message_id,
            };
            crate::respond(
                &self.forward.sink,
                prompt.request,
                Ok(serde_json::json!(inserted)),
            );
        }
        match event {
            AgentEvent::ChildUpdate { .. } => self.adopt_children(),
            AgentEvent::MessageEnd { .. }
            | AgentEvent::ToolExecutionEnd { .. }
            | AgentEvent::AgentEnd { .. } => {
                self.watch_goal();
                self.watch_workdir();
                self.watch_plan();
                if matches!(event, AgentEvent::ToolExecutionEnd { tool_name, .. } if tool_name == "todo")
                {
                    self.watch_claims();
                }
                if matches!(event, AgentEvent::AgentEnd { .. }) {
                    self.title();
                }
            }
            _ => {}
        }
    }
}

pub(crate) async fn forward_parent(
    mut events: tokio::sync::broadcast::Receiver<AgentEvent>,
    mut parent: Parent,
) {
    while let Some(next) = yi_runtime::next_event(&mut events).await {
        match next {
            Ok(event) => parent.reduce(&event),
            Err(gap) => {
                parent.forward.gap(gap.dropped);
                // Invariant: a lag may have dropped an insertion; a refusal beats a success naming it.
                let lost = "the update stream lagged past this prompt's insertion";
                crate::refuse(&parent.forward.sink, parent.ids.waiting(), -32603, lost);
            }
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
        UserContent::Blocks(blocks) => yi_types::message::join_text(blocks, " "),
    }
}
