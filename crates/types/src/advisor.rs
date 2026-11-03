use serde::{Deserialize, Serialize};

/// Advice severity (design §16): `Note`/`Warn` land as advisory entries,
/// `Hold` goes through the permission engine (§8) and degrades headless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdvisorySeverity {
    Note,
    Warn,
    Hold,
}

/// What kind of problem the advice names (design §16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdviceKind {
    Correction,
    Risk,
    Scope,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Advice {
    pub severity: AdvisorySeverity,
    pub kind: AdviceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub text: String,
}
