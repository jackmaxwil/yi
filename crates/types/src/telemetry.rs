//! One line per span in `<session file>.telemetry.jsonl`: the numbers a run is judged by,
//! keyed so a request, its tool calls and its turn correlate (plan 2026-09-05 §3.E).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpanKind {
    Request,
    Tool,
    Turn,
    Compaction,
    #[serde(untagged)]
    Other(String),
}

/// Every field a span may carry; a kind fills the ones it has. `class` is the error class
/// when the span ended badly, and stays absent on success.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub span: SpanKind,
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(default, flatten)]
    pub extra: Map<String, Value>,
}

impl Span {
    pub fn new(span: SpanKind, session: impl Into<String>) -> Self {
        Self {
            span,
            session: session.into(),
            turn: None,
            request: None,
            call: None,
            provider: None,
            model: None,
            ttft_ms: None,
            total_ms: None,
            input: None,
            output: None,
            cache_read: None,
            cache_write: None,
            cost_usd: None,
            tool: None,
            ms: None,
            ok: None,
            class: None,
            extra: Map::new(),
        }
    }
}
