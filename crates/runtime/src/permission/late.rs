//! Who answers an ask the classifier may judge, and when: `classifier.approval` puts it before the
//! person (`instant`), after `askTimeoutSecs` unanswered (`after-delay`), or never.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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

impl<'a> Late<'a> {
    /// The late ask `approver` makes of `call`, or none when it answers before the person.
    pub(super) fn of(
        approver: &'a crate::classifier::Approver,
        call: &'a crate::classifier::Call<'a>,
    ) -> Option<Self> {
        match approver.timing() {
            crate::classifier::Timing::AfterDelay(delay) => Some(Self {
                approver,
                call,
                delay,
            }),
            crate::classifier::Timing::Instant => None,
        }
    }
}

pub(super) enum Answered {
    Person(AskOutcome),
    Classifier(f64),
}

/// Invariant: only a confident allow answers for the person; anything else keeps the ask open
/// until they answer. With no person to ask, nobody answers for them.
pub(super) fn ask_or_judge(
    asker: Option<&Asker>,
    ask: &PermissionAsk<'_>,
    late: &Late<'_>,
) -> Answered {
    let Some(asker) = asker else {
        return Answered::Person(AskOutcome::Reject);
    };
    let (owned, asker) = (OwnedAsk::of(ask), Arc::clone(asker));
    let (sender, receiver) = std::sync::mpsc::channel();
    let claimed = Arc::new(AtomicBool::new(false));
    let theirs = Arc::clone(&claimed);
    std::thread::spawn(move || {
        let outcome = asker(&owned.ask());
        if !theirs.swap(true, Ordering::AcqRel) {
            let _ = sender.send(outcome);
        }
    });
    if let Ok(outcome) = receiver.recv_timeout(late.delay) {
        return Answered::Person(outcome);
    }
    let judged = late.approver.judge(late.call);
    if let crate::classifier::Judgement::Allow(safe) = judged {
        // Invariant: person and classifier claim one flag; a person who claimed it first decides,
        // a refusal included, and an answer after the classifier's claim finds the call decided.
        if !claimed.swap(true, Ordering::AcqRel) {
            return Answered::Classifier(safe);
        }
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
        let answered = match (approver.filter(|_| self.asker.is_some()), call) {
            (Some(approver), Some(call)) => match Late::of(approver, call) {
                Some(late) => self.late(ask, &late),
                None => match approver.judge(call) {
                    crate::classifier::Judgement::Allow(safe) => Some(Answered::Classifier(safe)),
                    _ => None,
                },
            },
            _ => None,
        };
        let answered = self.answered_or_person(ask, answered);
        let (outcome, classified) = self.settle_answered(&tool_call_id, ask, answered);
        (outcome, classified.is_some())
    }

    /// Settle an answered reviewable ask: a classifier allow is recorded as the
    /// classifier's and allows; a person's outcome settles as theirs and is returned.
    pub(super) fn settle_answered(
        &self,
        tool_call_id: &str,
        ask: &PermissionAsk<'_>,
        answered: Answered,
    ) -> (AskOutcome, Option<f64>) {
        match answered {
            Answered::Classifier(safe) => {
                self.settle(tool_call_id, ask, true, Answerer::Classifier);
                (AskOutcome::AllowOnce, Some(safe))
            }
            Answered::Person(outcome) => {
                let by = if self.asker.is_some() {
                    Answerer::User
                } else {
                    Answerer::Nobody
                };
                let allowed = matches!(outcome, AskOutcome::AllowOnce | AskOutcome::AllowAlways(_));
                self.settle(tool_call_id, ask, allowed, by);
                (outcome, None)
            }
        }
    }

    /// Whether [`PermissionBroker::confirm_judged`] gets an answer: a person, whom auto's
    /// classifier answers for only beside one, so a headless confirmation has no confirmer.
    pub fn can_confirm(&self) -> bool {
        self.can_ask()
    }

    /// Invariant: a surface whose prompt cannot close when its call settles elsewhere (a terminal
    /// blocked on stdin) never hands the ask to the classifier, or its reader takes the next answer.
    pub(super) fn late(&self, ask: &PermissionAsk<'_>, late: &Late<'_>) -> Option<Answered> {
        (self.prompts_close_on_settle.load(Ordering::Relaxed))
            .then(|| ask_or_judge(self.asker.as_ref(), ask, late))
    }
}
