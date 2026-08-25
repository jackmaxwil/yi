use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Goal status vocabulary (design G1, from codex `thread_goal.rs`). The
/// model may report `Complete`/`Blocked`; the host owns the rest (G2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
    #[serde(untagged)]
    Other(String),
}

impl GoalStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active)
    }

    /// Terminal states free the session for a new goal (G2 create rule).
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::BudgetLimited | Self::Complete)
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::UsageLimited => "usage_limited",
            Self::BudgetLimited => "budget_limited",
            Self::Complete => "complete",
            Self::Other(other) => other.as_str(),
        }
    }
}

/// One goal per session (design G1), stored as a session-store fact beside
/// the header — never a transcript entry, so compaction cannot lose it.
/// Timestamps are epoch milliseconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Goal {
    pub objective: String,
    pub status: GoalStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    pub tokens_used: u64,
    pub time_used_seconds: u64,
    pub created: u64,
    pub updated: u64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
