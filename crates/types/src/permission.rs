use serde::{Deserialize, Serialize};

/// Persisted session permission rules (design §8, §20 versioning).
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

/// The custom entry type a session journals each kept rule under; `--continue` replays them.
pub const PERMISSION_RULE_ENTRY: &str = "permission_rule";

impl crate::entry::CustomRecord for SessionPermissionRule {
    const TYPE: &'static str = PERMISSION_RULE_ENTRY;
}

/// ACP-shaped permission request/response.
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

/// The custom entry type a session journals each settled ask under.
pub const PERMISSION_ENTRY: &str = "permission";

/// Who settled an ask. `Nobody` is a denial because no one could be asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answerer {
    User,
    Reviewer,
    Classifier,
    Nobody,
    #[serde(untagged)]
    Other(String),
}

/// One settled permission ask as the session journals it: the question, the verdict, whose.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRecord {
    pub tool_call_id: String,
    pub title: String,
    pub description: String,
    pub allowed: bool,
    pub by: Answerer,
    #[serde(default, flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl crate::entry::CustomRecord for PermissionRecord {
    const TYPE: &'static str = PERMISSION_ENTRY;
}

/// Allow once, one "always" per grant (a bare one when the call has none), then reject, as
/// `(id, label, answer)`: the id is what an ACP client sends back, the answer each surface's own.
pub fn choices<'a, A>(
    grants: impl ExactSizeIterator<Item = &'a str>,
    once: A,
    always: impl Fn(usize) -> A,
    reject: A,
) -> Vec<(String, String, A)> {
    let mut listed = vec![("allow_once".into(), "Allow once".into(), once)];
    if grants.len() == 0 {
        listed.push(("allow_always".into(), "Always allow".into(), always(0)));
    }
    for (index, grant) in grants.enumerate() {
        // Grant 0 keeps the bare id, so a client that knows one always-option still works.
        let id = match index {
            0 => "allow_always".into(),
            _ => format!("allow_always_{index}"),
        };
        listed.push((id, format!("Always allow {grant}"), always(index)));
    }
    listed.push(("reject_once".into(), "Reject".into(), reject));
    listed
}

/// The answer a [`choices`] id names; an id no choice carries rejects.
pub fn chosen<A>(id: &str, once: A, always: impl Fn(usize) -> A, reject: A) -> A {
    match id {
        "allow_once" => once,
        "allow_always" => always(0),
        other => other
            .strip_prefix("allow_always_")
            .and_then(|index| index.parse().ok())
            .map_or(reject, always),
    }
}
