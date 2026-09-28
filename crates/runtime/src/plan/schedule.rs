//! After every op the engine starts each ready delegated todo admission lets through (D224).

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};

use serde_json::Value;
use yi_types::plan::canonical::canonical_digest;
use yi_types::plan::doc::{Plan, PlanId, PlanState, Todo, TodoLabel, TodoState};
use yi_types::plan::ledger::JournalRecord;

use super::artifact::Artifacts;
use super::ops::{Actor, Delta, ENGINE_AGENT, Op, OpRequest, Outcome, PlanEngine, PlanOpError};
use super::state::{IntentOutcome, KIND_LEFT, RootState, SUBMITTED_KEY, reduce, root_of};
use super::table::{admitted, ready_labels};

/// Invariant: only a contract refusal (per criteria stored) or a fuse refusal is kept.
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

fn contract_key(artifacts: &Artifacts, id: &PlanId, todo: &Todo) -> String {
    let digest = serde_json::to_value(&todo.contract)
        .ok()
        .and_then(|contract| canonical_digest(&contract).ok())
        .map(|digest| digest.hex())
        .unwrap_or_default();
    let stored: String = todo
        .contract
        .iter()
        .flat_map(|contract| contract.criteria())
        .map(
            |artifact| match artifacts.path(&artifact.digest).is_file() {
                true => '1',
                false => '0',
            },
        )
        .collect();
    format!(
        "{id}/{}/{}/{digest}/{stored}",
        todo.label,
        todo.attempt.get()
    )
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

    pub(super) fn stranded(&self, outcome: &IntentOutcome) -> bool {
        match outcome {
            IntentOutcome::Pending => true,
            IntentOutcome::Spawned { agent } => self.liveness.alive(agent) != Some(true),
        }
    }

    /// Invariant: submit and done are two commits, so a submit whose child is gone is done here.
    fn done_submitted(&self, root: &PlanId) -> Vec<Result<Outcome, Option<String>>> {
        let Some(reading) = self.store.journal(root).read().ok() else {
            return Vec::new();
        };
        let Ok(state) = reduce(&reading.records) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for (id, plan) in &state.plans {
            for todo in plan
                .todos
                .iter()
                .filter(|_| plan.state == PlanState::Active)
            {
                let TodoState::Running { by } = &todo.state else {
                    continue;
                };
                let submitted = reading.records.iter().any(|record| {
                    record.record.op == "submit"
                        && record.record.actor == ENGINE_AGENT
                        && record.record.plan == *id
                        && record.record.todo.as_ref() == Some(&todo.label)
                        && record.attempt == Some(todo.attempt)
                });
                let key = format!("{id}/{}/{}/submitted", todo.label, todo.attempt.get());
                if !submitted || self.liveness.alive(by) != Some(false) || self.refused.holds(&key)
                {
                    continue;
                }
                self.refused.insert(key);
                let output = todo.extra.get(SUBMITTED_KEY).and_then(Value::as_str);
                let request = OpRequest {
                    plan: Some(id.clone()),
                    actor: Actor::Engine,
                    op: Op::Done {
                        label: todo.label.clone(),
                        output: output.and_then(|url| url.parse().ok()),
                    },
                    request_id: None,
                    expected_revision: None,
                };
                out.push(self.apply_once(request).map_err(|error| {
                    let label = todo.label.as_str();
                    (!raced(&error)).then(|| {
                        format!("the engine could not complete {label:?} after its submit: {error}")
                    })
                }));
            }
        }
        out
    }

    fn start_ready(&self, root: &PlanId) -> Vec<Result<Outcome, Option<String>>> {
        let done = self.done_submitted(root);
        done.into_iter()
            .chain(self.startable(root).into_iter().map(|todo| {
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
            }))
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
                // Invariant: a stranded intent is repair's; a start would record the same refusal.
                let intent = state.intent_for(id, &label);
                if intent.is_some_and(|(_, intent)| self.stranded(&intent.outcome)) {
                    continue;
                }
                let contract = contract_key(&self.store.artifacts(id), id, todo);
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

    pub(super) fn standing(&self, plan: &Plan) -> (Vec<TodoLabel>, Standing) {
        let Some((records, state)) = root_of(&plan.id).ok().and_then(|root| {
            let records = self.store.journal(&root).read().ok()?.records;
            let state = reduce(&records).ok()?;
            Some((records, state))
        }) else {
            return (Vec::new(), Standing::default());
        };
        let now = admitted(plan, self.slots(&state));
        let (mut held, mut standing) = (Vec::new(), Standing::default());
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
                standing.unstarted.push((label, detail.to_owned()));
            }
        }
        standing.left = plan
            .todos
            .iter()
            .filter_map(|todo| left_to_owner(plan, todo, &records))
            .collect();
        (held, standing)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Standing {
    pub unstarted: Vec<(TodoLabel, String)>,
    pub left: Vec<(TodoLabel, String)>,
}

impl Standing {
    pub fn notices(&self) -> Vec<String> {
        let unstarted = self.unstarted.iter().map(|(label, detail)| {
            format!("the engine could not start {:?}: {detail}", label.as_str())
        });
        let left = self.left.iter().map(|(label, detail)| {
            format!("the engine left {:?} running: {detail}", label.as_str())
        });
        unstarted.chain(left).collect()
    }
}

fn left_to_owner(
    plan: &Plan,
    todo: &Todo,
    records: &[JournalRecord],
) -> Option<(TodoLabel, String)> {
    if todo.delegation.is_none() || !matches!(todo.state, TodoState::Running { .. }) {
        return None;
    }
    let left = records.iter().rev().find(|record| {
        record.record.plan == plan.id
            && record.record.todo.as_ref() == Some(&todo.label)
            && record.attempt == Some(todo.attempt)
            && record.record.op == KIND_LEFT
    })?;
    let detail = left.args.get("detail").and_then(Value::as_str)?;
    Some((todo.label.clone(), detail.to_owned()))
}
