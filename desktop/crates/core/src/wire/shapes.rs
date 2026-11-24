//! The standard ACP shapes this app decodes that `yi-types` holds no type for.

use serde::{Deserialize, Serialize};

/// A `select` session config option; `currentValue` is one of the `options` values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpConfigOption {
    pub config_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: AcpConfigKind,
    pub current_value: String,
    pub options: Vec<AcpConfigChoice>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpConfigKind {
    Select,
    #[serde(untagged)]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpConfigChoice {
    pub value: String,
    pub name: String,
}

/// ACP v2 stop reasons; unknown values round-trip via `Other`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpStopReason {
    EndTurn,
    MaxTokens,
    Refusal,
    Cancelled,
    #[serde(untagged)]
    Other(String),
}
