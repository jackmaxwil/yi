//! The `POST /v1/systemone` wire shapes a `classifier` sidecar speaks, and the ledger record of
//! each decision. Laya's server answers in this schema (its README, "Self-Hosting: HTTP Server").

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The custom entry type a session journals each classifier decision under.
pub const CLASSIFY_ENTRY: &str = "classify";

/// One request: a state and named typed questions, answered in one forward pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRequest {
    pub state: BTreeMap<String, Value>,
    pub questions: BTreeMap<String, Question>,
    /// The checkpoint to use; the server echoes the one it ran in `routing.model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Question {
    /// One option out of `criteria` (label to description).
    Choice {
        instructions: String,
        criteria: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionResponse {
    pub answers: BTreeMap<String, Answer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<Routing>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    /// The calibrated probability of the reported answer; the number to gate on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_confidence: Option<f64>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Routing {
    pub model: String,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// One classifier decision as the session journals it. `fired` says a pointer was delivered;
/// with no threshold set the classifier only records (shadow mode).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifyRecord {
    pub consumer: String,
    /// The typed message this answers: the first 12 hex of the sha256 of its bytes with ASCII
    /// whitespace collapsed and ASCII letters lowered, the id `evals/skill_labels.py` gives it.
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub latency_ms: u64,
    pub fired: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}
