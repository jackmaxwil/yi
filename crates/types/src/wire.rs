use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::entry::Entry;
use crate::record::LaneRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeaderKind {
    Header,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonlV4Header {
    pub kind: HeaderKind,
    pub version: u64,
    pub id: String,
    pub created_at: u64,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub legacy_parent_session_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "fact", rename_all = "lowercase")]
pub enum Fact {
    Name {
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Label {
        target_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(clippy::large_enum_variant, reason = "wire DTOs mirror the JSON; variants are parse-then-drop, boxing buys nothing")]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Mutation {
    Entry {
        #[serde(skip_serializing_if = "Option::is_none")]
        lane: Option<String>,
        #[serde(flatten)]
        entry: Entry,
    },
    Record {
        #[serde(flatten)]
        record: LaneRecord,
    },
    #[serde(rename_all = "camelCase")]
    Lane {
        seq: u64,
        lane: String,
        leaf_id: Option<String>,
    },
    Fact {
        seq: u64,
        #[serde(flatten)]
        fact: Fact,
    },
}
