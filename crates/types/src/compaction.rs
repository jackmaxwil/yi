use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Window ids chaining compactions (design P9): `first` is the session's
/// initial window, `previous` links windows into a chain, `id` names the
/// window opened by this compaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionWindow {
    pub first: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    pub id: String,
    pub number: u64,
}

/// Typed payload of a compaction entry's `details` value (design P8/P9).
/// File lists are cumulative across compactions.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionDetails {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modified_files: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<CompactionWindow>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
