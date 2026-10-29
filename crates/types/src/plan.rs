pub mod acceptance;
pub mod ask;
pub mod canonical;
#[macro_use]
pub mod ledger;
pub mod contract;
pub mod doc;
pub mod ids;
pub mod op;

/// The format-2 checkpoint schema, published to `.yi/schemas/plan.schema.json` on store open.
pub const PLAN_SCHEMA: &str = include_str!("plan/plan.schema.json");

use serde::{Deserialize, Serialize};

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
