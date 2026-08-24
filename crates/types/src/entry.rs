use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::message::{AgentMessage, Usage};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(clippy::large_enum_variant, reason = "wire DTOs mirror the JSON; variants are parse-then-drop, boxing buys nothing")]
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
}
