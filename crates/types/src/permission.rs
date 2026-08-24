use serde::{Deserialize, Serialize};

/// Persisted session permission rules (design M4; fx schema exemplar for
/// section 19 versioning). `digest` is recomputed from `canonical` on load and
/// never trusted from the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    Command,
    FileMutation,
    StructuredTool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleDecision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPermissionRule {
    pub id: u64,
    pub kind: RuleKind,
    pub canonical: String,
    pub display_identity: String,
    pub decision: RuleDecision,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPermissionState {
    pub version: u8,
    pub next_generation: u64,
    pub rules: Vec<SessionPermissionRule>,
}

impl Default for SessionPermissionState {
    fn default() -> Self {
        Self {
            version: 2,
            next_generation: 1,
            rules: Vec::new(),
        }
    }
}

/// ACP-shaped permission request/response (design M9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    pub title: String,
    pub description: String,
    pub subject: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionResponse {
    Selected { option: String },
    Cancelled,
}
