//! The `POST /v1/systemone` wire shapes the `classifier` speaks, and the ledger record of each
//! decision. TypeSafe's Jev answers in this schema through OpenRouter.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The custom entry type a session journals each classifier decision under.
pub const CLASSIFY_ENTRY: &str = "classify";

/// One request: a state and named typed questions, answered in one forward pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionRequest {
    /// Sent in insertion order: the order the measured question was asked in.
    pub state: serde_json::Map<String, Value>,
    pub questions: BTreeMap<String, Question>,
    /// The model to use; a server may echo the one it ran in `routing.model`.
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
    /// Yes or no; the answer's `noul` is P(yes).
    Noul { instructions: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecisionResponse {
    pub answers: BTreeMap<String, Answer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<Routing>,
    /// The model OpenRouter ran, dated (`typesafe/jev-1.13-20260917`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noul: Option<f64>,
    /// The calibrated probability of the reported answer; the number to gate on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_confidence: Option<f64>,
    /// Jev's `choice` answer: each option's probability. With no `answer_confidence`, the
    /// chosen option's entry is the number to gate on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probabilities: Option<BTreeMap<String, serde_json::Number>>,
    /// Jev's confidence in the answer as a whole, beside `probabilities`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<serde_json::Number>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Routing {
    pub model: String,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// One classifier decision as the session journals it. `fired` says a pointer was delivered.
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

impl crate::entry::CustomRecord for ClassifyRecord {
    const TYPE: &'static str = CLASSIFY_ENTRY;
}
