use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Ledger entry kinds (design P10): no `skill`, no refine events in Yi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HarnessKind {
    Prompt,
    Memory,
    Subagent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HarnessScope {
    Local,
    Global,
}

fn default_path() -> String {
    "general".to_owned()
}

fn default_source() -> String {
    "agent".to_owned()
}

fn default_version() -> u64 {
    1
}

/// One reusable prompt note, memory, or subagent record. Disk shape is shared
/// with `harness.py`: snake_case fields, JSON keyed `entries.{kind}.{id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessEntry {
    pub id: String,
    pub kind: HarnessKind,
    pub title: String,
    pub content: String,
    #[serde(default = "default_path")]
    pub path: String,
    #[serde(default = "HarnessEntry::default_scope")]
    pub scope: HarnessScope,
    #[serde(default)]
    pub reference: Map<String, Value>,
    #[serde(default)]
    pub arguments: Map<String, Value>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default = "default_version")]
    pub version: u64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl HarnessEntry {
    fn default_scope() -> HarnessScope {
        HarnessScope::Local
    }
}
