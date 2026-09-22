//! After every op the engine starts each ready delegated todo admission lets through (D224).

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use serde_json::Value;
use yi_types::plan::canonical::canonical_digest;
use yi_types::plan::doc::{Plan, PlanId, PlanState, Todo, TodoLabel};

use super::ops::{Actor, Delta, Op, OpRequest, Outcome, PlanEngine, PlanOpError, admitted};
use super::state::{IntentOutcome, RootState, reduce, root_of};
use super::table::ready_labels;

/// Invariant: only a contract or fuse refusal is kept, keyed by its cause; the rest retry.
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

fn contract_key(id: &PlanId, todo: &Todo) -> String {
    let digest = serde_json::to_value(&todo.contract)
        .ok()
        .and_then(|contract| canonical_digest(&contract).ok())
        .map(|digest| digest.hex())
        .unwrap_or_default();
    format!("{id}/{}/{}/{digest}", todo.label, todo.attempt.get())
}

fn fuse_key(contract: &str, spent: u32) -> String {
    format!("{contract}/fuse/{spent}")
}

pub(super) fn raced(error: &PlanOpError) -> bool {
    matches!(
        error,
        PlanOpError::IllegalStep { .. }
            | PlanOpError::Admission(_)
            | PlanOpError::UnmetEdge { .. }
            | PlanOpError::NeedsReconciliation { .. }
    )
}

struct Startable {
    plan: PlanId,
    label: TodoLabel,
    contract: String,
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
                Err(Some(notice)) => outcome.notices.push(notice),
                Err(None) => {}
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

    /// The probe tick's backstop; a sibling session's roots in the same directory are its own.
    pub fn dispatch_ready_in(&self, roots: &[PlanId]) {
        if !self.delegate.hosts() {
            return;
        }
        for root in roots {
            let _journaled_and_nobody_to_tell = self.start_ready(root);
        }
    }

    fn start_ready(&self, root: &PlanId) -> Vec<Result<Outcome, Option<String>>> {
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
                    match &error {
                        PlanOpError::Contract { .. } => self.refused.insert(todo.contract),
                        PlanOpError::SpawnCeilingExhausted { spent, .. } => {
                            self.refused.insert(fuse_key(&todo.contract, spent.get()));
                        }
                        other if raced(other) => return None,
                        _ => {}
                    }
                    Some(format!(
                        "the engine could not start {:?}: {error}",
                        todo.label.as_str()
                    ))
                })
            })
            .collect()
    }

    pub(super) fn family(&self, root: &PlanId) -> Option<RootState> {
        let reading = self.store.journal(root).read().ok()?;
        reduce(&reading.records).ok()
    }

    fn startable(&self, root: &PlanId) -> Vec<Startable> {
        let Some(state) = self.family(root) else {
            return Vec::new();
        };
        let spent = state.plan(root).map_or(0, |plan| plan.spawns().get());
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
                let contract = contract_key(id, todo);
                if self.refused.holds(&contract) || self.refused.holds(&fuse_key(&contract, spent))
                {
                    continue;
                }
                slots = slots.saturating_sub(1);
                out.push(Startable {
                    plan: id.clone(),
                    label,
                    contract,
                });
            }
        }
        out
    }

    pub(super) fn standing(&self, plan: &Plan) -> (Vec<TodoLabel>, Vec<String>) {
        let Some((records, state)) = root_of(&plan.id).ok().and_then(|root| {
            let records = self.store.journal(&root).read().ok()?.records;
            let state = reduce(&records).ok()?;
            Some((records, state))
        }) else {
            return (Vec::new(), Vec::new());
        };
        let now = admitted(plan, self.slots(&state));
        let (mut held, mut notices) = (Vec::new(), Vec::new());
        for label in ready_labels(plan) {
            if !now.contains(&label) {
                held.push(label);
                continue;
            }
            let Some(todo) = plan.todo(&label).filter(|todo| todo.delegation.is_some()) else {
                continue;
            };
            let refusal = records.iter().rev().find_map(|record| {
                let own = record.record.plan == plan.id
                    && record.record.todo.as_ref() == Some(&label)
                    && record.attempt == Some(todo.attempt)
                    && record.record.op == "start";
                own.then(|| record.record.extra.get("refusal"))
                    .flatten()
                    .and_then(|refusal| refusal.get("detail"))
                    .and_then(Value::as_str)
            });
            if let Some(detail) = refusal {
                notices.push(format!(
                    "the engine could not start {:?}: {detail}",
                    label.as_str()
                ));
            }
        }
        (held, notices)
    }
}
