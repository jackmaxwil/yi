//! Who answers an ask the classifier may judge, and when: `classifier.approval` puts it before the
//! person (`instant`), after `askTimeoutSecs` unanswered (`after-delay`), or never.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use yi_types::event::AgentEvent;

use super::{Answerer, AskOutcome, Asker, PermissionAsk, PermissionBroker, PermissionMode};

struct OwnedAsk {
    title: String,
    description: String,
    patch: Option<String>,
    changes: Vec<PathBuf>,
    grants: Vec<yi_permission::Grant>,
    tool_call_id: Option<String>,
}

impl OwnedAsk {
    fn of(ask: &PermissionAsk<'_>) -> Self {
        Self {
            title: ask.title.to_owned(),
            description: ask.description.to_owned(),
            patch: ask.patch.map(str::to_owned),
            changes: ask.changes.to_vec(),
            grants: ask.grants.to_vec(),
            tool_call_id: ask.tool_call_id.map(str::to_owned),
        }
    }

    fn ask(&self) -> PermissionAsk<'_> {
        PermissionAsk {
            title: &self.title,
            description: &self.description,
            patch: self.patch.as_deref(),
            changes: &self.changes,
            grants: &self.grants,
            tool_call_id: self.tool_call_id.as_deref(),
        }
    }
}

pub(super) struct Late<'a> {
    pub(super) approver: &'a crate::classifier::Approver,
    pub(super) call: &'a crate::classifier::Call<'a>,
    pub(super) delay: std::time::Duration,
}

pub(super) enum Answered {
    Person(AskOutcome),
    Classifier(f64),
}

/// Invariant: only a confident allow answers for the person; anything else keeps the ask open
/// until they answer. With no person to ask, the classifier answers at once.
pub(super) fn ask_or_judge(
    asker: Option<&Asker>,
    ask: &PermissionAsk<'_>,
    late: &Late<'_>,
) -> Answered {
    let Some(asker) = asker else {
        return match late.approver.judge(late.call) {
            crate::classifier::Judgement::Allow(safe) => Answered::Classifier(safe),
            _ => Answered::Person(AskOutcome::Reject),
        };
    };
    let (owned, asker) = (OwnedAsk::of(ask), Arc::clone(asker));
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _gone_if_the_classifier_answered = sender.send(asker(&owned.ask()));
    });
    if let Ok(outcome) = receiver.recv_timeout(late.delay) {
        return Answered::Person(outcome);
    }
    let judged = late.approver.judge(late.call);
    // A person who answered while the sidecar was judging decides, a refusal included.
    if let Ok(outcome) = receiver.try_recv() {
        return Answered::Person(outcome);
    }
    if let crate::classifier::Judgement::Allow(safe) = judged {
        return Answered::Classifier(safe);
    }
    Answered::Person(receiver.recv().unwrap_or(AskOutcome::Reject))
}

impl PermissionBroker {
    /// [`PermissionBroker::confirm`] with auto mode's classifier first; the bool says it answered.
    pub fn confirm_judged(
        &self,
        ask: &PermissionAsk<'_>,
        call: Option<&crate::classifier::Call<'_>>,
    ) -> (AskOutcome, bool) {
        let ordinal = self
            .confirms
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let tool_call_id = format!("confirm-{ordinal}");
        // The asker's prompt carries the id it settles under, so a front end can close it.
        let ask = &PermissionAsk {
            tool_call_id: Some(&tool_call_id),
            ..*ask
        };
        let _ = self.events.send(AgentEvent::PermissionRequested {
            tool_call_id: tool_call_id.clone(),
            title: ask.title.to_owned(),
            description: ask.text(),
        });
        let approver = self
            .approver
            .get()
            .filter(|_| self.mode() == PermissionMode::Auto);
        let judged = match (approver, call) {
            (Some(approver), Some(call)) => match approver.timing() {
                crate::classifier::Timing::Instant => approver.judge(call),
                crate::classifier::Timing::AfterDelay(delay) => {
                    let late = Late {
                        approver,
                        call,
                        delay,
                    };
                    match self
                        .late(&late)
                        .map(|late| ask_or_judge(self.asker.as_ref(), ask, late))
                    {
                        Some(Answered::Classifier(safe)) => {
                            crate::classifier::Judgement::Allow(safe)
                        }
                        Some(Answered::Person(outcome)) => {
                            let allowed = matches!(
                                outcome,
                                AskOutcome::AllowOnce | AskOutcome::AllowAlways(_)
                            );
                            let by = if self.asker.is_some() {
                                Answerer::User
                            } else {
                                Answerer::Nobody
                            };
                            self.settle(&tool_call_id, ask, allowed, by);
                            return (outcome, false);
                        }
                        None => crate::classifier::Judgement::Undecided,
                    }
                }
            },
            _ => crate::classifier::Judgement::Undecided,
        };
        if let crate::classifier::Judgement::Allow(_) = judged {
            self.settle(&tool_call_id, ask, true, Answerer::Classifier);
            return (AskOutcome::AllowOnce, true);
        }
        let outcome = self
            .asker
            .as_ref()
            .map_or(AskOutcome::Reject, |asker| asker(ask));
        let by = if self.asker.is_some() {
            Answerer::User
        } else {
            Answerer::Nobody
        };
        let allowed = matches!(outcome, AskOutcome::AllowOnce | AskOutcome::AllowAlways(_));
        self.settle(&tool_call_id, ask, allowed, by);
        (outcome, false)
    }

    /// Whether [`PermissionBroker::confirm_judged`] gets an answer: a person, or auto's classifier.
    pub fn can_confirm(&self) -> bool {
        self.can_ask() || (self.mode() == PermissionMode::Auto && self.approver.get().is_some())
    }

    /// Invariant: a prompt that cannot close when its call settles elsewhere (a terminal
    /// blocked on stdin) is never answered for, or its reader would take the next prompt's answer.
    pub(super) fn late<'a>(&self, late: &'a Late<'a>) -> Option<&'a Late<'a>> {
        (self.asker.is_none() || self.prompts_close_on_settle.load(Ordering::Relaxed))
            .then_some(late)
    }
}
