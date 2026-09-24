use std::sync::Arc;

use yi_types::message::{AgentMessage, StopReason};
use yi_types::subagent::{ChildExit, FailClass};

use super::{INTERRUPTED, Standing, Step, SubagentHost, last_assistant_text, preview};
use crate::family::Cause;
use crate::session::AgentSession;

/// How a settled run ended, read off its last assistant message; a run a cancel or the
/// deadline stopped at a message boundary ends on a tool call with no request after it.
fn exit_of(session: &AgentSession) -> (ChildExit, Option<String>) {
    let messages = session.messages();
    let last = messages.iter().rev().find_map(|message| match message {
        AgentMessage::Assistant {
            stop_reason,
            error_message,
            ..
        } => Some((stop_reason, error_message)),
        _ => None,
    });
    let out_of_clock = session.deadline().is_some_and(|clock| clock.winding_down());
    match last {
        Some((StopReason::Error, message)) => (
            ChildExit::Failed {
                class: FailClass::Provider,
            },
            Some(
                message
                    .clone()
                    .unwrap_or_else(|| "child run ended with an error".to_owned()),
            ),
        ),
        Some((StopReason::Aborted, _)) => (ChildExit::Interrupted, Some(INTERRUPTED.to_owned())),
        Some((StopReason::ToolUse, _)) if session.cancelled() => {
            (ChildExit::Interrupted, Some("cancelled".to_owned()))
        }
        Some((StopReason::ToolUse, _)) if out_of_clock => (
            ChildExit::Failed {
                class: FailClass::Deadline,
            },
            Some("the deadline ended the run before its task did".to_owned()),
        ),
        _ => (ChildExit::Completed, None),
    }
}

impl SubagentHost {
    pub(super) async fn run_child(
        self: Arc<Self>,
        child_id: String,
        session_name: String,
        prompt: String,
        context: Option<String>,
        session: Arc<AgentSession>,
        requested: u64,
    ) {
        // Invariant: scope rides the first user message, never a trusted block.
        let content = match context {
            Some(block) => format!("[task from parent]\n\n{block}\n\n{prompt}"),
            None => format!("[task from parent]\n\n{prompt}"),
        };
        if let Ok(mut children) = self.children.lock()
            && let Some(record) = children.get_mut(&child_id)
            && record.step(Step::Started)
        {
            children.touch(&child_id, crate::family::Cause::Started);
        }
        let outcome =
            session.prompt_requested(crate::session::user_message(&content), Some(requested));
        let mut ended = outcome.err().map(|error| {
            let class = FailClass::RefusedSpawn;
            (ChildExit::Failed { class }, Some(error.to_string()))
        });
        if ended.is_none() {
            session.wait_idle().await;
            ended = Some(exit_of(&session));
        }
        let (exit, error) = ended.unwrap_or((ChildExit::Completed, None));
        self.conclude(&child_id, &session_name, &session, exit, error);
    }

    /// A run's ending, unless it is a service's crash and the record was respawned instead.
    pub(super) fn conclude(
        self: &Arc<Self>,
        child_id: &str,
        session_name: &str,
        session: &AgentSession,
        exit: ChildExit,
        error: Option<String>,
    ) {
        let Some((exit, error)) = self.respawn(child_id, exit, error) else {
            return;
        };
        // Invariant: held from before the exit is visible, so `busy` never reads a gap.
        let _settling = crate::plan::finish::Settling::hold(self);
        // A retired child has no record left: `retire` already published its terminal update,
        // so a second one, a notice or a bill would only echo a closed slot.
        let messages = session.messages();
        // a child that ended on `ask_user` is asking its parent, not finishing (D165).
        let question = crate::family::pending_question(&messages);
        let (mut ended, mut juror) = (None, false);
        if let Ok(mut children) = self.children.lock()
            && let Some(record) = children.get_mut(child_id)
        {
            record.concluding = false;
            let billed = std::mem::replace(&mut record.attributed, messages.len());
            if record.step(Step::Exit(exit, error.clone())) {
                let service = matches!(record.standing, Standing::Service(_));
                let (replied, cause) = (record.replied, Cause::ended(exit, question.is_some()));
                juror = matches!(record.standing, Standing::Juror);
                let epoch = children.touch(child_id, cause);
                ended = Some((replied, service, billed, epoch));
            }
        }
        let Some((replied, service, billed, epoch)) = ended else {
            return;
        };
        if exit == ChildExit::Completed {
            for message in messages.get(billed..).unwrap_or_default() {
                if let AgentMessage::Assistant {
                    stop_reason, usage, ..
                } = message
                    && !matches!(stop_reason, StopReason::Error | StopReason::Aborted)
                {
                    (self.options.attribute)(usage);
                }
            }
        }
        self.publish(child_id);
        if session.status() == crate::session::Status::Running {
            self.resume(child_id);
        }
        if juror {
            return;
        }
        let taken = match exit {
            ChildExit::Completed if question.is_some() || service => false,
            ChildExit::Completed
            | ChildExit::Failed { .. }
            | ChildExit::Interrupted
            | ChildExit::Other => self.finish_taken(session_name, exit, error.clone()),
            ChildExit::Reaped | ChildExit::Repossessed => false,
        };
        if taken {
            return;
        }
        let verb = crate::family::read_exit(Some(exit)).verb;
        let notice = match (exit, question) {
            (ChildExit::Completed, Some(question)) => format!(
                "[subagent {session_name} ({child_id}) asks: {question}]\nanswer with rlm.send(\"{session_name}\", \"…\", followup=True)"
            ),
            // An idle service has not ended: it waits on its inbox and there is nothing to reap.
            (ChildExit::Completed, None) if service => return,
            (ChildExit::Completed, None) => {
                let answer = last_assistant_text(&messages)
                    .map(|text| preview(&text))
                    .unwrap_or_else(|| "(no final answer text)".to_owned());
                let silent = if replied {
                    ""
                } else {
                    "; it sent you no message"
                };
                format!(
                    "[subagent {session_name} ({child_id}) {verb}{silent}]\nLast answer: {answer}\n{}",
                    crate::affordance::next(
                        "rlm.run",
                        &[yi_types::graph::CHILD_FINISHED],
                        session_name
                    )
                )
            }
            (ChildExit::Failed { .. }, _) => format!(
                "[subagent {session_name} ({child_id}) {verb}]\n{}",
                error.unwrap_or_default()
            ),
            _ => format!("[subagent {session_name} ({child_id}) {verb}]"),
        };
        let (host, key) = (Arc::downgrade(self), child_id.to_owned());
        let news = Arc::new(move || host.upgrade().is_some_and(|host| host.unseen(&key, epoch)));
        (self.options.notice)(&notice, Some(news));
    }

    /// Another run on a concluded record reads running again, and one reader concludes it.
    pub(super) fn resume(self: &Arc<Self>, key: &str) {
        let claimed = self.children.lock().ok().and_then(|mut children| {
            let record = children.get_mut(key).filter(|record| !record.concluding)?;
            record.concluding = true;
            record.step(Step::Resumed);
            let claimed = (record.session_name.clone(), Arc::clone(&record.session));
            children.touch(key, Cause::Started);
            Some(claimed)
        });
        let Some((name, session)) = claimed else {
            return;
        };
        self.publish(key);
        let (host, key) = (Arc::clone(self), key.to_owned());
        tokio::spawn(async move {
            session.wait_idle().await;
            let (exit, error) = exit_of(&session);
            host.conclude(&key, &name, &session, exit, error);
        });
    }

    fn unseen(&self, key: &str, epoch: u64) -> bool {
        self.children
            .lock()
            .is_ok_and(|children| children.get(key).is_some_and(|record| record.seen < epoch))
    }

    /// The owner read these endings through `result`, `wait` or `status`: their notices are old.
    pub(crate) fn saw(&self, only: Option<&str>) {
        if let Ok(mut children) = self.children.lock() {
            for record in children.values_mut() {
                let named = only.is_none_or(|name| record.session_name == name);
                if named && record.exit.is_some() {
                    record.seen = record.changed_at_epoch;
                }
            }
        }
    }
}
