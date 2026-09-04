use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EntryOrder {
    #[default]
    NewestFirst,
    OldestFirst,
}

#[derive(Debug, Clone, Default)]
pub struct EntryQuery {
    pub entry_type: Option<&'static str>,
    pub custom_type: Option<String>,
    pub order: EntryOrder,
    pub limit: Option<usize>,
    pub after_seq: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct BranchBounds {
    pub start: Option<String>,
    pub stop_at_type: Option<&'static str>,
    pub stop_at_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct RecordQuery {
    pub lane: Option<String>,
    pub record_type: Option<&'static str>,
    pub run_id: Option<String>,
    pub operation_kind: Option<&'static str>,
    pub after_seq: Option<u64>,
    pub order: EntryOrder,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LogOptions {
    pub after_seq: Option<u64>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryHit {
    pub entry_id: String,
    pub entry_type: String,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStats {
    pub message_count: u64,
    pub cached_tokens: i64,
    pub uncached_tokens: i64,
    pub total_tokens: i64,
    pub cost_total: f64,
}

impl SessionStats {
    pub fn zero() -> Self {
        Self {
            message_count: 0,
            cached_tokens: 0,
            uncached_tokens: 0,
            total_tokens: 0,
            cost_total: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanePointer {
    pub lane: String,
    pub leaf_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadata {
    pub id: String,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CreateOptions {
    pub id: Option<String>,
    pub parent_session_id: Option<String>,
    pub metadata: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkPosition {
    Before,
    At,
}

#[derive(Debug, Clone)]
pub enum ForkScope {
    Branch {
        entry_id: Option<String>,
        position: Option<ForkPosition>,
    },
    Tree,
}

impl Default for ForkScope {
    fn default() -> Self {
        Self::Branch {
            entry_id: None,
            position: None,
        }
    }
}
