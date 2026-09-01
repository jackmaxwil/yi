use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::doc::{PlanId, TodoLabel, TodoStateName};

pub const PLAN_OP_ENTRY_TYPE: &str = "plan_op";

/// The `custom{plan_op}` entry payload: one applied op, appended to the owning
/// session. The plan file carries the state and nothing else carries when it
/// moved, so every duration in the ledger's yield is a difference of `at`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanOpRecord {
    pub plan: PlanId,
    pub op: String,
    pub actor: String,
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo: Option<TodoLabel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<TodoStateName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<TodoStateName>,
    /// How many todos the plan carried once the op had applied, so a discovery
    /// ratio needs no second pass over the files a superseded generation left.
    pub todos: u32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
