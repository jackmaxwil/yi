use std::sync::Arc;

use yi_types::message::{AgentMessage, Content, StopReason};
use yi_types::subagent::{ChildExit, FailClass};

use super::{INTERRUPTED, Standing, Step, SubagentHost, preview};
use crate::family::Cause;
use crate::session::AgentSession;

/// How a settled run ended, read off its last assistant message.
fn exit_of(session: &AgentSession) -> (ChildExit, Option<String>) {
    if session
        .kernel_service()
        .is_some_and(|kernel| kernel.take_death())
    {
        let class = FailClass::KernelDeath;
        return (
            ChildExit::Failed { class },
            Some("its kernel died under a cell".to_owned()),
        );
    }
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

fn failed(class: FailClass, error: impl std::fmt::Display) -> (ChildExit, Option<String>) {
    (ChildExit::Failed { class }, Some(error.to_string()))
}

fn schema_failure(
    session: &AgentSession,
    messages: &[AgentMessage],
    start: usize,
) -> Option<String> {
    let run = messages.get(start..).unwrap_or_default();
    let mut prompts = run.iter().filter(|message| {
        matches!(
            message,
            AgentMessage::User { .. } | AgentMessage::Custom { .. }
        )
    });
    prompts.next()?;
    if prompts
        .next_back()
        .is_some_and(|message| !crate::mail::is_chase(message))
    {
        return None;
    }
    let schema = crate::schema::Schema::from_value(session.request_shape()?.schema?).ok()?;
    crate::schema::extract(&super::answer_text(run).unwrap_or_default())
        .map_or_else(Some, |value| schema.validate(&value).err())
}

fn has_tool_blocks(messages: &[AgentMessage]) -> bool {
    messages.iter().any(|message| match message {
        AgentMessage::ToolResult { .. } => true,
        AgentMessage::Assistant { content, .. } => content
            .iter()
            .any(|block| matches!(block, Content::ToolCall { .. })),
        _ => false,
    })
}

async fn complete(
    session: &AgentSession,
    mut ended: Option<(ChildExit, Option<String>)>,
    start: usize,
) -> (ChildExit, Option<String>) {
    if ended.is_none() {
        session.wait_idle().await;
        ended = Some(exit_of(session));
    }
    let (exit, error) = ended.unwrap_or((ChildExit::Completed, None));
    if exit != ChildExit::Completed {
        return (exit, error);
    }
    let messages = session.messages();
    let Some(error) = schema_failure(session, &messages, start) else {
        return (exit, error);
    };
    let repair = format!("schema retry: {error}; reply with JSON only.");
    let mut prompt = repair.clone();
    let mut cold = false;
    if session.model().api == "anthropic-messages" {
        let converted = yi_context::convert_to_llm(&messages);
        cold = has_tool_blocks(&converted);
        if cold {
            let history = yi_context::serialize_conversation(&converted);
            prompt = format!("{history}\n\n{repair}");
        }
    }
    let (tools, turn_cap) = (session.tools(), session.turn_cap());
    if cold {
        session.seed_messages(Vec::new());
    }
    session.set_turn_cap(1);
    session.set_tools(Vec::new());
    let mut repaired = session
        .prompt_message(crate::session::task(&prompt))
        .err()
        .map(|error| failed(FailClass::Other, error));
    if repaired.is_none() {
        session.wait_idle().await;
        repaired = Some(exit_of(session));
    }
    session.set_tools(tools);
    session.restore_turn_cap(turn_cap);
    if cold {
        let repair_messages = session.messages();
        session.seed_messages(messages.into_iter().chain(repair_messages).collect());
    }
    repaired.unwrap_or((ChildExit::Completed, None))
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
        // Incident: told only the doctrine, a child sent to find what its parent was told walked
        // the tree and gave up in two runs of three; a wall that denies history is never told it.
        let walled = session.wall().denies_history();
        let header = match walled {
            true => "[task from parent]",
            false => {
                "[task from parent]\n[its transcript: history://parent/since/0 · one line of it: grep with path=history://parent]"
            }
        };
        // Invariant: scope rides the first user message, never a trusted block.
        let content = match context {
            Some(block) => format!("{header}\n\n{block}\n\n{prompt}"),
            None => format!("{header}\n\n{prompt}"),
        };
        if let Ok(mut children) = self.children.lock()
            && let Some(record) = children.get_mut(&child_id)
            && record.step(Step::Started)
        {
            children.touch(&child_id, crate::family::Cause::Started);
        }
        let start = session.messages().len();
        let outcome = session.prompt_requested(crate::session::task(&content), Some(requested));
        let ended = outcome
            .err()
            .map(|error| failed(FailClass::RefusedSpawn, error));
        let (exit, error) = complete(&session, ended, start).await;
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
        let mut ended = None;
        if let Ok(mut children) = self.children.lock()
            && let Some(record) = children.get_mut(child_id)
        {
            record.concluding = false;
            record.concluded_runs = session.runs().1;
            let billed = std::mem::replace(&mut record.attributed, messages.len());
            if record.step(Step::Exit(exit, error.clone())) {
                let service = matches!(record.standing, Standing::Service(_));
                let (replied, cause) = (record.replied, Cause::ended(exit));
                let trail = (record.token_count(), record.trail.clone());
                let ending = (record.exit.unwrap_or(exit), record.error.clone());
                let epoch = children.touch(child_id, cause);
                ended = Some((replied, service, billed, epoch, trail, ending));
            }
        }
        let Some((replied, service, billed, epoch, (tokens, trail), (last, why))) = ended else {
            return;
        };
        if !(service && last == ChildExit::Completed) {
            self.trail_ended(trail.as_ref(), session_name, child_id, (last, why, tokens));
        }
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
        let steered = exit == ChildExit::Completed
            && self.chase_open_requests(session_name, session, &messages);
        self.publish(child_id);
        if session.status() == crate::session::Status::Running {
            self.resume(child_id);
        }
        if steered {
            return;
        }
        let taken = match exit {
            ChildExit::Completed if service => false,
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
        let notice = match exit {
            // An idle service has not ended: it waits on its inbox and there is nothing to reap.
            ChildExit::Completed if service => return,
            ChildExit::Completed => {
                let answer = super::answer_text(&messages)
                    .map(|text| preview(&text, session_name))
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
            ChildExit::Failed { .. } => format!(
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
    pub(crate) fn resume(self: &Arc<Self>, key: &str) {
        let claimed = self.children.lock().ok().and_then(|mut children| {
            let record = children.get_mut(key).filter(|record| {
                !record.concluding && record.session.runs().0 > record.concluded_runs
            })?;
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
        let start = session.messages().len();
        tokio::spawn(async move {
            let (exit, error) = complete(&session, None, start).await;
            host.conclude(&key, &name, &session, exit, error);
        });
    }

    /// Invariant: a derived `stuck` moves no record, so a newly stuck member moves the epoch.
    pub(crate) fn mark_stuck(&self) {
        let views = self.states();
        let stuck: Vec<&str> = views
            .iter()
            .filter(|view| view.state == crate::family::MemberState::Stuck)
            .map(|view| view.name.as_str())
            .collect();
        let Ok(mut latched) = self.stuck.lock() else {
            return;
        };
        latched.retain(|name| stuck.contains(&name.as_str()));
        let fresh: Vec<&str> = stuck
            .into_iter()
            .filter(|name| latched.insert((*name).to_owned()))
            .collect();
        drop(latched);
        if let Ok(mut children) = self.children.lock() {
            for name in fresh {
                if let Ok(key) = Self::key_of(&children, name) {
                    children.touch(&key, Cause::Stuck);
                }
            }
        }
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
