//! After every op the engine starts each ready delegated todo admission lets through (D224).

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use yi_types::plan::doc::{PlanId, PlanState, TodoLabel};

use super::ops::{Actor, Delta, Op, OpRequest, Outcome, PlanEngine, admitted};
use super::state::{IntentOutcome, RootState, reduce, root_of};

/// Invariant: a refused engine start is journaled once per attempt and process, not per op.
#[derive(Default)]
pub(super) struct Refused(Mutex<HashSet<String>>);

impl Refused {
    fn holds(&self, key: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(key)
    }

    fn insert(&self, key: String) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key);
    }
}

struct Startable {
    plan: PlanId,
    label: TodoLabel,
    key: String,
}

impl PlanEngine {
    pub(super) fn dispatch_ready(&self, outcome: &mut Outcome) {
        let Ok(root) = root_of(&outcome.plan.id) else {
            return;
        };
        let tried = self.start_ready(&root);
        if tried.is_empty() {
            return;
        }
        for started in tried {
            match started {
                Ok(started) => {
                    outcome.spawned.extend(started.spawned);
                    outcome.notices.extend(started.notices);
                }
                Err(notice) => outcome.notices.push(notice),
            }
        }
        // Invariant: every attempt committed records, so the plan, ready and held are re-derived.
        let now = self.family(&root).and_then(|state| {
            self.conclude(&outcome.plan.id, &state, &[], Delta::default())
                .ok()
        });
        if let Some(now) = now {
            outcome.plan = now.plan;
            outcome.ready = now.ready;
            outcome.held = now.held;
            outcome
                .dispatched
                .retain(|label| outcome.ready.contains(label));
        }
    }

    pub fn dispatch_ready_all(&self) {
        if !self.delegate.hosts() {
            return;
        }
        for root in self.store.roots().unwrap_or_default() {
            let _journaled_and_nobody_to_tell = self.start_ready(&root);
        }
    }

    fn start_ready(&self, root: &PlanId) -> Vec<Result<Outcome, String>> {
        self.startable(root)
            .into_iter()
            .map(|todo| {
                let request = OpRequest {
                    plan: Some(todo.plan),
                    actor: Actor::Engine,
                    op: Op::Start {
                        label: todo.label.clone(),
                    },
                    request_id: None,
                    expected_revision: None,
                };
                self.apply_once(request).map_err(|error| {
                    if error.is_recordable() {
                        self.refused.insert(todo.key);
                    }
                    format!(
                        "the engine could not start {:?}: {error}",
                        todo.label.as_str()
                    )
                })
            })
            .collect()
    }

    fn family(&self, root: &PlanId) -> Option<RootState> {
        let reading = self.store.journal(root).read().ok()?;
        reduce(&reading.records).ok()
    }

    fn startable(&self, root: &PlanId) -> Vec<Startable> {
        let Some(state) = self.family(root) else {
            return Vec::new();
        };
        let mut slots = self.slots(&state);
        let mut out = Vec::new();
        for (id, plan) in &state.plans {
            if plan.state != PlanState::Active {
                continue;
            }
            for label in admitted(plan, slots) {
                let Some(todo) = plan.todo(&label).filter(|todo| todo.delegation.is_some()) else {
                    continue;
                };
                // Invariant: a pending intent is repair's; a start would record the same refusal.
                let intent = state.intent_for(id, &label);
                if intent.is_some_and(|(_, intent)| intent.outcome == IntentOutcome::Pending) {
                    continue;
                }
                let key = format!("{id}/{label}/{}", todo.attempt.get());
                if self.refused.holds(&key) {
                    continue;
                }
                slots = slots.saturating_sub(1);
                out.push(Startable {
                    plan: id.clone(),
                    label,
                    key,
                });
            }
        }
        out
    }
}
