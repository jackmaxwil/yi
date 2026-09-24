use yi_types::event::AgentEvent;
use yi_types::message::{AgentMessage, Content};
use yi_types::subagent::ChildExit;

use super::{ChildActivity, ChildId, ChildRecord, ChildUpdate};
use crate::family::Phase;
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
    /// The child's own verdict on its work, which its run's ending then carries.
    Failed(String),
    /// A repossession owns the ending from here: the run's own exit is mute.
    Repossess,
    Pending(String),
    Exit(ChildExit, Option<String>),
    /// A service's next incarnation took the record: the run before it is no ending.
    Respawn,
    /// Another run started on a concluded record: it is live again until its own ending.
    Resumed,
}

/// A child's own `failure` is the only producer of this class so far (D215).
fn red_check() -> ChildExit {
    ChildExit::Failed {
        class: yi_types::subagent::FailClass::RedCheck,
    }
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
    pub(crate) fn token_count(&self) -> u64 {
        self.token_count
    }

    /// The turns this incarnation is billed and counted for: a respawned service keeps its
    /// predecessor's transcript, which was already charged to the lease that ended with it.
    pub(crate) fn billable<'a>(&self, messages: &'a [AgentMessage]) -> &'a [AgentMessage] {
        messages.get(self.billed_from..).unwrap_or_default()
    }

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
            flag: None,
        }
    }

    /// Invariant: the one writer of a record's lifecycle fields. `false` means nothing a
    /// client can see moved; an exit is refused while a repossession owns the ending.
    pub(crate) fn step(&mut self, step: Step<'_>) -> bool {
        match step {
            Step::Started => {
                let queued = self.phase == Phase::Queued;
                if queued {
                    self.phase = Phase::Live;
                }
                queued
            }
            Step::Repossess => {
                self.phase = Phase::Repossessing;
                false
            }
            Step::Pending(reason) => {
                self.phase = Phase::Pending;
                self.exit = None;
                // Its run was stopped: a card left mid-tool would show work nothing is doing.
                self.activity = ChildActivity::Waiting;
                self.error = Some(reason);
                true
            }
            Step::Respawn => {
                self.phase = Phase::Queued;
                (self.exit, self.error, self.replied) = (None, None, false);
                // The lease was returned on the last incarnation's count and drawn again.
                (self.token_count, self.tool_use_count) = (0, 0);
                self.activity = ChildActivity::Waiting;
                true
            }
            Step::Resumed if self.exit.is_some() => {
                (self.exit, self.error, self.replied) = (None, None, false);
                self.phase = Phase::Live;
                true
            }
            Step::Resumed => false,
            Step::Replied => !std::mem::replace(&mut self.replied, true),
            Step::Held(reason) => {
                self.error = Some(reason);
                true
            }
            // Invariant: a verdict is not an ending, so a live run keeps its absent exit: one
            // run publishes one ending, and no child declares itself out of its own grace.
            Step::Failed(text) => {
                self.error = Some(text);
                match self.exit {
                    None => self.phase = Phase::Failed,
                    Some(_) => self.exit = Some(red_check()),
                }
                true
            }
            Step::Exit(exit, _)
                if exit != ChildExit::Repossessed
                    && matches!(self.phase, Phase::Repossessing | Phase::Pending) =>
            {
                false
            }
            // A child that sent its own `failure` ends failed, whatever its last turn looked
            // like; its own words stay the cause.
            Step::Exit(ChildExit::Completed, _) if self.phase == Phase::Failed => {
                self.phase = Phase::Live;
                self.exit = Some(red_check());
                self.activity = ChildActivity::Waiting;
                true
            }
            Step::Exit(exit, error) => {
                self.phase = Phase::Live;
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
