//! Recovery (plan section 5.6): reconstruction first, reconciliation second, effects never.

use std::path::PathBuf;

use yi_types::plan::doc::{AgentId, PlanId, TodoLabel, TodoState};
use yi_types::plan::ledger::{EffectId, JournalRecord};

use super::journal::Damage;
use super::state::{IntentOutcome, RootState, reduce};
use super::store::{PlanStore, StoreError};

pub trait Liveness: Send + Sync {
    fn alive(&self, agent: &AgentId) -> Option<bool>;
}

pub struct Unknown;

impl Liveness for Unknown {
    fn alive(&self, _agent: &AgentId) -> Option<bool> {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub plan: PlanId,
    pub label: TodoLabel,
    pub agent: Option<AgentId>,
    pub effect: Option<EffectId>,
    pub evidence: String,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}/{} needs reconciliation: {}",
            self.plan, self.label, self.evidence
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recovered {
    pub state: RootState,
    pub regenerated: Vec<PlanId>,
    pub torn: Option<PathBuf>,
    pub findings: Vec<Finding>,
}

/// The journal read once: its state, the records it reduced from, and the torn tail set aside.
pub fn reconstruct(
    store: &PlanStore,
    root: &PlanId,
) -> Result<(RootState, Vec<JournalRecord>, Option<PathBuf>), StoreError> {
    let journal = store.journal(root);
    let reading = journal.read()?;
    let torn = match reading.damage {
        None => None,
        Some(Damage::TornTail { offset, .. }) => Some(journal.set_aside(offset, &store.nonce())?),
        Some(Damage::Corrupt {
            offset,
            seq,
            reason,
            ..
        }) => {
            return Err(StoreError::RecoveryRequired {
                root: root.clone(),
                seq: seq.map(|seq| seq.get()),
                offset,
                reason,
            });
        }
    };
    let state = reduce(&reading.records).map_err(|source| StoreError::Reduce {
        root: root.clone(),
        source,
    })?;
    Ok((state, reading.records, torn))
}

pub fn reconcile(state: &RootState, liveness: &dyn Liveness) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (id, plan) in &state.plans {
        for todo in &plan.todos {
            let TodoState::Running { by } = &todo.state else {
                continue;
            };
            if todo.delegation.is_none() || liveness.alive(by) == Some(true) {
                continue;
            }
            let effect = state
                .intent_for(id, &todo.label)
                .map(|(effect, _)| effect.clone());
            findings.push(Finding {
                plan: id.clone(),
                label: todo.label.clone(),
                agent: Some(by.clone()),
                effect,
                evidence: match liveness.alive(by) {
                    Some(false) => format!("child {by} has no live process and no durable result"),
                    Some(true) => String::new(),
                    None => format!("child {by} cannot be shown alive and left no durable result"),
                },
            });
        }
    }
    for (effect, intent) in &state.intents {
        let already = findings
            .iter()
            .any(|finding| finding.plan == intent.plan && finding.label == intent.label);
        if already {
            continue;
        }
        let evidence = match &intent.outcome {
            IntentOutcome::Pending => {
                format!("spawn intent {effect} has no result; the spawn may or may not have run")
            }
            IntentOutcome::Spawned { agent } => {
                format!(
                    "child {agent} was spawned for intent {effect} but the start never committed"
                )
            }
        };
        findings.push(Finding {
            plan: intent.plan.clone(),
            label: intent.label.clone(),
            agent: match &intent.outcome {
                IntentOutcome::Spawned { agent } => Some(agent.clone()),
                IntentOutcome::Pending => None,
            },
            effect: Some(effect.clone()),
            evidence,
        });
    }
    findings
}

pub fn run(
    store: &PlanStore,
    root: &PlanId,
    liveness: &dyn Liveness,
) -> Result<Recovered, StoreError> {
    let (state, _records, torn) = reconstruct(store, root)?;
    let regenerated = store.checkpoint_family(&state)?;
    let findings = reconcile(&state, liveness);
    Ok(Recovered {
        state,
        regenerated,
        torn,
        findings,
    })
}
