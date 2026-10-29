use serde::{Deserialize, Deserializer, Serialize};
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

/// The line's object as written; the tree fields are read out of it, never mirrored beside it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct UnknownEntry(Map<String, Value>);

impl UnknownEntry {
    fn id(&self) -> &str {
        self.0.get("id").and_then(Value::as_str).unwrap_or_default()
    }

    fn parent_id(&self) -> Option<&str> {
        match self.0.get("parentId") {
            Some(Value::String(parent)) => Some(parent),
            _ => None,
        }
    }

    fn seq(&self) -> u64 {
        self.0
            .get("seq")
            .and_then(Value::as_u64)
            .unwrap_or_default()
    }

    fn assign(&mut self, parent_id: Option<String>, seq: u64, timestamp: u64) {
        let parent = parent_id.map_or(Value::Null, Value::from);
        self.0.insert("parentId".to_owned(), parent);
        self.0.insert("seq".to_owned(), Value::from(seq));
        self.0
            .insert("timestamp".to_owned(), Value::from(timestamp));
    }

    fn set_seq(&mut self, seq: u64) {
        self.0.insert("seq".to_owned(), Value::from(seq));
    }
}

impl<'de> Deserialize<'de> for UnknownEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let raw = crate::wire::unknown_tag(deserializer, "type", &ENTRY_TYPES)?;
        if raw.get("id").and_then(Value::as_str).is_none()
            || raw.get("seq").and_then(Value::as_u64).is_none()
        {
            return Err(D::Error::custom("an unknown entry needs an id and a seq"));
        }
        if !matches!(
            raw.get("parentId"),
            None | Some(Value::Null) | Some(Value::String(_))
        ) {
            return Err(D::Error::custom(
                "an unknown entry's parentId is not a string",
            ));
        }
        Ok(Self(raw))
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
            Self::Other(unknown) => unknown.id(),
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
            Self::Other(unknown) => unknown.parent_id(),
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
            Self::Other(unknown) => unknown.seq(),
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
            Self::Other(unknown) => (unknown.0.get("timestamp"))
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
            Self::Other(_) => "unknown",
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
            Self::Other(unknown) => unknown.assign(new_parent_id, new_seq, new_timestamp),
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
            Self::Other(unknown) => unknown.set_seq(new_seq),
        }
    }
}
