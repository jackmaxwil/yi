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

/// The closed vocabulary every error carries, rendered `kind:detail` into a span's `class`
/// and counted by the live lane; a class absent from the last runs is a finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorClass {
    Tool(String),
    Provider(String),
    TransportHttp(u16),
    TransportTimeout,
    TransportProxy,
    TransportBody,
    TransportOs(i32),
    TransportTls,
    TransportClosed,
    RefusalUnknownModel,
    RefusalNoKey,
    RefusalPoolFull,
    RefusalConfig,
    RefusalWalled,
    RefusalLane,
    Invariant(String),
    Harness(String),
    Other(String),
}

impl ErrorClass {
    /// A provider's error text names its transport class when it can; the rest is the provider.
    pub fn from_provider_text(text: &str) -> Self {
        let lower = text.to_lowercase();
        if lower.contains("no credential for provider") {
            return Self::RefusalNoKey;
        }
        if let Some(rest) = lower.split("http ").nth(1)
            && let Some(code) = rest.split(|c: char| !c.is_ascii_digit()).next()
            && let Ok(status) = code.parse::<u16>()
        {
            return Self::TransportHttp(status);
        }
        if lower.contains("timed out") || lower.contains("timeout") {
            return Self::TransportTimeout;
        }
        if lower.contains("proxy") {
            return Self::TransportProxy;
        }
        if lower.contains("body over") {
            return Self::TransportBody;
        }
        // Incident: `Bad address (os error 14)`, a dead TLS handshake and a stream closed
        // mid-body all read `provider:error` on ledger row 0017 (issue #256).
        if let Some(rest) = lower.split("os error ").nth(1)
            && let Some(code) = rest.split(|c: char| !c.is_ascii_digit()).next()
            && let Ok(number) = code.parse::<i32>()
        {
            return Self::TransportOs(number);
        }
        if lower.contains("tls") || lower.contains("certificate") {
            return Self::TransportTls;
        }
        if [
            "connection closed",
            "stream closed",
            "connection reset",
            "connection failed",
            "broken pipe",
            "decoding chunks",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        {
            return Self::TransportClosed;
        }
        Self::Provider("error".to_owned())
    }
}

impl ErrorClass {
    /// The wire failed, not the provider's answer: a rerun cannot duplicate anything.
    pub fn is_transport(&self) -> bool {
        matches!(
            self,
            Self::TransportHttp(_)
                | Self::TransportTimeout
                | Self::TransportProxy
                | Self::TransportBody
                | Self::TransportOs(_)
                | Self::TransportTls
                | Self::TransportClosed
        )
    }
}

impl std::fmt::Display for ErrorClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tool(kind) => write!(f, "tool:{kind}"),
            Self::Provider(kind) => write!(f, "provider:{kind}"),
            Self::TransportHttp(status) => write!(f, "transport:http_{status}"),
            Self::TransportTimeout => f.write_str("transport:timeout"),
            Self::TransportProxy => f.write_str("transport:proxy"),
            Self::TransportBody => f.write_str("transport:body"),
            Self::TransportOs(code) => write!(f, "transport:os_{code}"),
            Self::TransportTls => f.write_str("transport:tls"),
            Self::TransportClosed => f.write_str("transport:closed"),
            Self::RefusalUnknownModel => f.write_str("refusal:unknown_model"),
            Self::RefusalNoKey => f.write_str("refusal:no_key"),
            Self::RefusalPoolFull => f.write_str("refusal:pool_full"),
            Self::RefusalConfig => f.write_str("refusal:config"),
            Self::RefusalWalled => f.write_str("refusal:walled"),
            Self::RefusalLane => f.write_str("refusal:lane"),
            Self::Invariant(row) => write!(f, "invariant:{row}"),
            Self::Harness(what) => write!(f, "harness:{what}"),
            Self::Other(what) => write!(f, "other:{what}"),
        }
    }
}
