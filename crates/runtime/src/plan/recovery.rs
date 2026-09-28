//! Recovery (plan section 5.6): reconstruction first, reconciliation second, effects never.

use std::path::PathBuf;

use yi_types::plan::doc::{AgentId, PlanId, TodoLabel, TodoState};
use yi_types::plan::ledger::{EffectId, JournalRecord};

use super::journal::Damage;
use super::ops::{Actor, Op, Outcome, PlanEngine, PlanOpError, Resolution};
use super::state::{IntentOutcome, RootState, SUBMITTED_KEY, reduce, root_of};
use super::store::{PlanStore, StoreError};
use yi_types::plan::doc::TouchCount;
use yi_types::plan::ledger::RequestId;

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
            let left = match todo.extra.get(SUBMITTED_KEY).and_then(|url| url.as_str()) {
                Some(url) => format!("its submitted product {url}, which no done accepted"),
                None => "no durable result".to_owned(),
            };
            findings.push(Finding {
                plan: id.clone(),
                label: todo.label.clone(),
                agent: Some(by.clone()),
                effect,
                evidence: match liveness.alive(by) {
                    Some(false) => format!("child {by} has no live process and left {left}"),
                    Some(true) => String::new(),
                    None => format!("child {by} cannot be shown alive and left {left}"),
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

impl PlanEngine {
    pub(super) fn repair(
        &self,
        plan: Option<PlanId>,
        resolutions: Vec<Resolution>,
        actor: &Actor,
        request: RequestId,
        expected: Option<TouchCount>,
    ) -> Result<Outcome, PlanOpError> {
        let id = self.resolve(plan)?;
        let root = root_of(&id)?;
        let recovered = run(&self.store, &root, &*self.liveness)?;
        let mut notices: Vec<String> = Vec::new();
        if let Some(torn) = &recovered.torn {
            notices.push(format!(
                "a torn journal tail was set aside at {}",
                torn.display()
            ));
        }
        notices.push(format!(
            "regenerated {} checkpoint(s) from the journal",
            recovered.regenerated.len()
        ));
        notices.extend(recovered.findings.iter().map(ToString::to_string));
        notices.extend(self.drop_left_staging(&root));
        for resolution in &resolutions {
            let found = recovered
                .findings
                .iter()
                .any(|finding| finding.plan == id && finding.label == resolution.label);
            if !found {
                return Err(PlanOpError::NotReconcilable {
                    label: resolution.label.clone(),
                });
            }
        }
        let mut outcome = if resolutions.is_empty() {
            self.view(Some(id), true)?
        } else {
            self.framed(
                Some(id),
                actor,
                Op::Repair { resolutions },
                request,
                expected,
                Default::default(),
            )?
        };
        outcome.notices.splice(0..0, notices);
        Ok(outcome)
    }
}
