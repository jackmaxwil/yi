use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::message::{AgentMessage, Usage};

/// A payload the session log stores as `custom{TYPE}`; every view of that log reads it back.
pub trait CustomRecord: Serialize + serde::de::DeserializeOwned {
    const TYPE: &'static str;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "wire DTOs mirror the JSON; variants are parse-then-drop, boxing buys nothing"
)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Entry {
    #[serde(rename_all = "camelCase")]
    Message {
        id: String,
        message: AgentMessage,
        #[serde(skip_serializing_if = "Option::is_none")]
        terminate: Option<bool>,
        parent_id: Option<String>,
        seq: u64,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    ModelChange {
        id: String,
        provider: String,
        model_id: String,
        parent_id: Option<String>,
        seq: u64,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    ThinkingLevelChange {
        id: String,
        thinking_level: String,
        parent_id: Option<String>,
        seq: u64,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    ActiveToolsChange {
        id: String,
        active_tool_names: Vec<String>,
        parent_id: Option<String>,
        seq: u64,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    Compaction {
        id: String,
        summary: String,
        retained_tail: Vec<AgentMessage>,
        tokens_before: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        parent_id: Option<String>,
        seq: u64,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    BranchSummary {
        id: String,
        from_id: String,
        summary: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        parent_id: Option<String>,
        seq: u64,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    Custom {
        id: String,
        custom_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<Value>,
        parent_id: Option<String>,
        seq: u64,
        timestamp: u64,
    },
    /// A `type` this build does not know, written by a newer build or by Pi; kept as written.
    #[serde(untagged)]
    Other(UnknownEntry),
}

const ENTRY_TYPES: [&str; 7] = [
    "message",
    "model_change",
    "thinking_level_change",
    "active_tools_change",
    "compaction",
    "branch_summary",
    "custom",
];

/// The tree fields are read out so the entry stays in its branch; `raw` is the line's object.
#[derive(Debug, Clone, PartialEq)]
pub struct UnknownEntry {
    id: String,
    parent_id: Option<String>,
    seq: u64,
    raw: Map<String, Value>,
}

impl UnknownEntry {
    pub fn type_name(&self) -> &str {
        self.raw.get("type").and_then(Value::as_str).unwrap_or("")
    }

    fn set(&mut self, key: &str, value: Value) {
        if let Some(slot) = self.raw.get_mut(key) {
            *slot = value;
        } else {
            self.raw.insert(key.to_owned(), value);
        }
    }
}

impl Serialize for UnknownEntry {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for UnknownEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let raw = Map::deserialize(deserializer)?;
        let tag = raw.get("type").and_then(Value::as_str);
        if tag.is_none_or(|tag| ENTRY_TYPES.contains(&tag)) {
            return Err(D::Error::custom("a known entry type that does not decode"));
        }
        let id = raw.get("id").and_then(Value::as_str);
        let seq = raw.get("seq").and_then(Value::as_u64);
        let (Some(id), Some(seq)) = (id, seq) else {
            return Err(D::Error::custom("an unknown entry needs an id and a seq"));
        };
        let parent_id = match raw.get("parentId") {
            None | Some(Value::Null) => None,
            Some(Value::String(parent)) => Some(parent.clone()),
            Some(_) => {
                return Err(D::Error::custom(
                    "an unknown entry's parentId is not a string",
                ));
            }
        };
        Ok(Self {
            id: id.to_owned(),
            parent_id,
            seq,
            raw,
        })
    }
}

impl Entry {
    pub fn id(&self) -> &str {
        match self {
            Self::Message { id, .. }
            | Self::ModelChange { id, .. }
            | Self::ThinkingLevelChange { id, .. }
            | Self::ActiveToolsChange { id, .. }
            | Self::Compaction { id, .. }
            | Self::BranchSummary { id, .. }
            | Self::Custom { id, .. } => id,
            Self::Other(unknown) => &unknown.id,
        }
    }

    pub fn parent_id(&self) -> Option<&str> {
        match self {
            Self::Message { parent_id, .. }
            | Self::ModelChange { parent_id, .. }
            | Self::ThinkingLevelChange { parent_id, .. }
            | Self::ActiveToolsChange { parent_id, .. }
            | Self::Compaction { parent_id, .. }
            | Self::BranchSummary { parent_id, .. }
            | Self::Custom { parent_id, .. } => parent_id.as_deref(),
            Self::Other(unknown) => unknown.parent_id.as_deref(),
        }
    }

    pub fn seq(&self) -> u64 {
        match self {
            Self::Message { seq, .. }
            | Self::ModelChange { seq, .. }
            | Self::ThinkingLevelChange { seq, .. }
            | Self::ActiveToolsChange { seq, .. }
            | Self::Compaction { seq, .. }
            | Self::BranchSummary { seq, .. }
            | Self::Custom { seq, .. } => *seq,
            Self::Other(unknown) => unknown.seq,
        }
    }

    pub fn timestamp(&self) -> u64 {
        match self {
            Self::Message { timestamp, .. }
            | Self::ModelChange { timestamp, .. }
            | Self::ThinkingLevelChange { timestamp, .. }
            | Self::ActiveToolsChange { timestamp, .. }
            | Self::Compaction { timestamp, .. }
            | Self::BranchSummary { timestamp, .. }
            | Self::Custom { timestamp, .. } => *timestamp,
            Self::Other(unknown) => (unknown.raw.get("timestamp"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
        }
    }

    pub fn type_name(&self) -> &str {
        match self {
            Self::Message { .. } => "message",
            Self::ModelChange { .. } => "model_change",
            Self::ThinkingLevelChange { .. } => "thinking_level_change",
            Self::ActiveToolsChange { .. } => "active_tools_change",
            Self::Compaction { .. } => "compaction",
            Self::BranchSummary { .. } => "branch_summary",
            Self::Custom { .. } => "custom",
            Self::Other(unknown) => unknown.type_name(),
        }
    }

    pub fn custom_type(&self) -> Option<&str> {
        match self {
            Self::Custom { custom_type, .. } => Some(custom_type),
            _ => None,
        }
    }

    pub fn assign(&mut self, new_parent_id: Option<String>, new_seq: u64, new_timestamp: u64) {
        match self {
            Self::Message {
                parent_id,
                seq,
                timestamp,
                ..
            }
            | Self::ModelChange {
                parent_id,
                seq,
                timestamp,
                ..
            }
            | Self::ThinkingLevelChange {
                parent_id,
                seq,
                timestamp,
                ..
            }
            | Self::ActiveToolsChange {
                parent_id,
                seq,
                timestamp,
                ..
            }
            | Self::Compaction {
                parent_id,
                seq,
                timestamp,
                ..
            }
            | Self::BranchSummary {
                parent_id,
                seq,
                timestamp,
                ..
            }
            | Self::Custom {
                parent_id,
                seq,
                timestamp,
                ..
            } => {
                *parent_id = new_parent_id;
                *seq = new_seq;
                *timestamp = new_timestamp;
            }
            Self::Other(unknown) => {
                unknown.set(
                    "parentId",
                    new_parent_id.clone().map_or(Value::Null, Value::from),
                );
                unknown.parent_id = new_parent_id;
                unknown.set("seq", Value::from(new_seq));
                unknown.seq = new_seq;
                unknown.set("timestamp", Value::from(new_timestamp));
            }
        }
    }

    pub fn set_seq(&mut self, new_seq: u64) {
        match self {
            Self::Message { seq, .. }
            | Self::ModelChange { seq, .. }
            | Self::ThinkingLevelChange { seq, .. }
            | Self::ActiveToolsChange { seq, .. }
            | Self::Compaction { seq, .. }
            | Self::BranchSummary { seq, .. }
            | Self::Custom { seq, .. } => *seq = new_seq,
            Self::Other(unknown) => {
                unknown.set("seq", Value::from(new_seq));
                unknown.seq = new_seq;
            }
        }
    }
}
