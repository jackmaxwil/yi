use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const RECLAIM_ENTRY_TYPE: &str = "reclaim";

/// The `custom{reclaim}` entry: tool results every later request shows as a one-line
/// placeholder; the stored results are untouched, and the cut is never undone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReclaimRecord {
    pub items: Vec<Reclaimed>,
    /// `breakeven`: the reads it saves outweigh the rewrite; `expired`: the cache had lapsed.
    pub reason: String,
    /// Requests the session had sent when the cut was made.
    pub turn: u64,
    /// Estimated tokens the cut drops from every later request.
    pub dropped: u64,
    /// Estimated tokens after the first cut result, which the next request writes again.
    pub rewritten: u64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reclaimed {
    /// Not unique alone: GLM's leaked calls are `leak-0`, `leak-1`, … in every message.
    pub tool_call_id: String,
    /// The first 16 hex digits of the sha256 of the result's text, which tells such calls apart.
    pub hash: String,
    /// The entry holding the whole result, which `history://self/<id>` serves.
    pub entry_id: Option<String>,
}

impl crate::entry::CustomRecord for ReclaimRecord {
    const TYPE: &'static str = RECLAIM_ENTRY_TYPE;
}
