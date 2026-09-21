use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, Content};
use yi_types::subagent::ChildExit;

use super::{ChildActivity, ChildId, ChildRecord, ChildUpdate};
use crate::session::AgentSession;

/// A child's session as a reader holds it: its events, transcript, model and status. It has
/// no abort, prompt or send, so stopping a child is the host's `interrupt` and nothing else.
#[derive(Clone)]
pub struct ChildFeed(std::sync::Arc<AgentSession>);

impl ChildFeed {
    pub fn of(session: std::sync::Arc<AgentSession>) -> Self {
        Self(session)
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AgentEvent> {
        self.0.subscribe()
    }

    pub fn store(&self) -> Option<yi_session::SharedSession> {
        self.0.store()
    }

    pub fn messages(&self) -> Vec<AgentMessage> {
        self.0.messages()
    }

    pub fn model(&self) -> yi_types::model::Model {
        self.0.model()
    }

    pub fn status(&self) -> crate::session::Status {
        self.0.status()
    }

    pub async fn wait_idle(&self) {
        self.0.wait_idle().await;
    }
}

/// Everything that moves a record: the child's own events, its first poll, a message it sent
/// up, a lane that would not settle, and the one exit it ends on.
pub(crate) enum Step<'a> {
    Event(&'a AgentEvent),
    Started,
    Replied,
    Held(String),
    Exit(ChildExit, Option<String>),
}

pub(super) fn preview(text: &str) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() > 240 {
        let capped: String = compact.chars().take(240).collect();
        format!("{capped}…")
    } else {
        compact
    }
}

impl ChildRecord {
    pub(crate) fn update(&self, child_id: &str) -> ChildUpdate {
        ChildUpdate {
            id: ChildId(child_id.to_owned()),
            name: self.session_name.clone(),
            status: crate::family::read_exit(self.exit).status,
            activity: self.activity,
            tool_use_count: self.tool_use_count,
            token_count: self.token_count,
            answer_preview: self.answer_preview.clone(),
            error: self.error.clone(),
            exit: self.exit,
        }
    }

    /// Invariant: the one writer of a record's lifecycle fields. `false` means nothing a
    /// client can see moved; an exit is refused while a repossession owns the ending.
    pub(crate) fn step(&mut self, step: Step<'_>) -> bool {
        match step {
            Step::Started => std::mem::take(&mut self.queued),
            Step::Replied => !std::mem::replace(&mut self.replied, true),
            Step::Held(reason) => {
                self.error = Some(reason);
                true
            }
            Step::Exit(..) if self.repossessing => false,
            Step::Exit(exit, error) => {
                self.queued = false;
                self.exit = Some(exit);
                self.activity = ChildActivity::Waiting;
                self.error = error;
                true
            }
            Step::Event(event) => self.fold(event),
        }
    }

    fn fold(&mut self, event: &AgentEvent) -> bool {
        match event {
            AgentEvent::ToolExecutionStart { .. } => {
                self.tool_use_count = self.tool_use_count.saturating_add(1);
                self.activity = ChildActivity::Executing;
            }
            AgentEvent::ToolExecutionEnd { .. }
            | AgentEvent::MessageStart {
                message: AgentMessage::Assistant { .. },
            } => self.activity = ChildActivity::Writing,
            AgentEvent::MessageEnd {
                message: AgentMessage::Assistant { usage, content, .. },
            } => {
                let tokens = u64::try_from(usage.total_tokens).unwrap_or(0);
                self.token_count = self.token_count.saturating_add(tokens);
                self.activity = ChildActivity::Waiting;
                let text: String = content
                    .iter()
                    .filter_map(|block| match block {
                        Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    self.answer_preview = Some(preview(&text));
                }
            }
            _ => return false,
        }
        true
    }
}
