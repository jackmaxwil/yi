use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostTier {
    pub input_tokens_above: u64,
    pub input: Number,
    pub output: Number,
    pub cache_read: Number,
    pub cache_write: Number,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCost {
    pub input: Number,
    pub output: Number,
    pub cache_read: Number,
    pub cache_write: Number,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tiers: Option<Vec<CostTier>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub name: String,
    pub api: String,
    pub provider: String,
    pub base_url: String,
    pub reasoning: bool,
    pub input: Vec<String>,
    pub cost: ModelCost,
    pub context_window: u64,
    pub max_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compat: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_level_map: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Splits [`LlmContext::system_prompt`] into independently cacheable blocks.
/// A control character: prompt assembly strips it from environment text.
pub const SYSTEM_BLOCK_SEPARATOR: &str = "\u{1d}";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmContext {
    pub system_prompt: String,
    pub messages: Vec<crate::message::AgentMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDef>>,
}
