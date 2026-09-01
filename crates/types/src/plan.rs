pub mod doc;
pub mod ids;
pub mod ledger;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Monotonic plan revision. Reviewer findings and audits cite it, so a
/// verdict is never adjudicated against a standard that has since grown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct PlanVersion(pub u64);

impl PlanVersion {
    pub fn bump(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TaskId(pub String);

impl TaskId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stored states only: Ready is derived from deps ([`Plan::frontier`]), never
/// persisted. The blocked reason lives beside the state on the task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Pending,
    Running,
    Done,
    Blocked,
    #[serde(untagged)]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    /// What must be true when this task is done — the completion standard's
    /// inventory line; weakening it mid-run is gated, adding is free.
    pub acceptance: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<TaskId>,
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// Consecutive red done-claims, the escalation ladder's only input. It
    /// rides the task because the ladder must survive a resume unlaundered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub red_count: Option<u8>,
    /// Hex digest of the last rejection, compared for equality only — a
    /// hasher change costs one missed repeat hint, never a wrong refusal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub red_fingerprint: Option<String>,
    /// One structural move buys one further done-claim at a rung that refuses
    /// them. It rides the fact, or a resume mints an unearned attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readmit: Option<bool>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One subtask of a split proposal — the only surface on which the model
/// shapes topology; ids, deps, and state are written by the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtaskSpec {
    pub title: String,
    pub acceptance: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    /// State keys this child reads; a key only a sibling writes is a
    /// read-before-write hazard, since siblings carry no ordering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<String>,
    /// Declared write set; disjointness across siblings gates parallelism.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One per session, stored as a fact beside the header like the goal, so
/// compaction cannot lose it. A plan without an Active goal is inert
/// structure; the goal is what arms unattended continuation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub version: PlanVersion,
    pub tasks: Vec<Task>,
    pub created: u64,
    pub updated: u64,
    /// Invariant: unvalidated, like every sibling id here — a validating
    /// newtype on a durable JSONL field makes one bad pointer fail the whole
    /// session file. Set means a [`crate::plan::doc::PlanId`] file is truth.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Plan {
    pub fn task(&self, id: &TaskId) -> Option<&Task> {
        self.tasks.iter().find(|task| &task.id == id)
    }

    /// Ready = Pending with every dep Done. Unknown dep ids never make a
    /// task ready (validation rejects them at write time anyway).
    pub fn frontier(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|task| {
                task.state == TaskState::Pending
                    && task.deps.iter().all(|dep| {
                        self.task(dep)
                            .is_some_and(|dep_task| dep_task.state == TaskState::Done)
                    })
            })
            .collect()
    }

    pub fn is_finished(&self) -> bool {
        !self.tasks.is_empty() && self.tasks.iter().all(|task| task.state == TaskState::Done)
    }
}

#[cfg(test)]
mod tests {
    use crate::wire::{Fact, Mutation};

    #[test]
    fn a_malformed_doc_pointer_leaves_the_fact_loadable() -> Result<(), Box<dyn std::error::Error>>
    {
        let line = concat!(
            r#"{"kind":"fact","seq":1,"fact":"plan","plan":{"version":1,"tasks":[],"#,
            r#""created":0,"updated":0,"doc":"NOT_a.plan.id"}}"#
        );
        let Mutation::Fact {
            fact: Fact::Plan { plan },
            ..
        } = serde_json::from_str(line)?
        else {
            return Err("expected a plan fact".into());
        };
        assert_eq!(plan.doc.as_deref(), Some("NOT_a.plan.id"));
        assert!(crate::plan::doc::PlanId::new("NOT_a.plan.id").is_err());
        assert!(serde_json::to_string(&plan)?.contains("NOT_a.plan.id"));
        Ok(())
    }
}
