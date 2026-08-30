use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

/// Invariant: variant order is the ladder — [`Ord`], [`Effort::ALL`] and every
/// clamp read it, and no second ordered list of levels exists anywhere.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Off,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    XHigh,
    Max,
}

/// A `--thinking`, config, or wire value that names no [`Effort`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownEffort(pub String);

impl std::fmt::Display for UnknownEffort {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "unknown thinking level {}", self.0)
    }
}

impl std::error::Error for UnknownEffort {}

impl Effort {
    pub const ALL: [Self; 7] = [
        Self::Off,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::XHigh,
        Self::Max,
    ];

    /// Tiers a cycle shortcut refuses to enter; the picker gates them.
    pub fn is_advanced(self) -> bool {
        matches!(self, Self::XHigh | Self::Max)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl std::fmt::Display for Effort {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for Effort {
    type Err = UnknownEffort;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|effort| effort.as_str() == value)
            .ok_or_else(|| UnknownEffort(value.to_owned()))
    }
}

impl Model {
    /// Invariant: the advertised levels, low to high, never empty — a model
    /// whose every level is `null`-mapped falls back to [`Effort::Off`], so
    /// callers may take `.first()` / `.last()` with no empty case to invent.
    pub fn supported_efforts(&self) -> Vec<Effort> {
        if !self.reasoning {
            return vec![Effort::Off];
        }
        let map = self.thinking_level_map.as_ref();
        let supported: Vec<Effort> = Effort::ALL
            .into_iter()
            .filter(
                |effort| match map.and_then(|map| map.get(effort.as_str())) {
                    Some(Value::Null) => false,
                    None => !effort.is_advanced(),
                    Some(_) => true,
                },
            )
            .collect();
        if supported.is_empty() {
            vec![Effort::Off]
        } else {
            supported
        }
    }

    /// Snaps up before down, so an unsupported request is never quietly
    /// answered with less thinking while a higher rung exists.
    pub fn clamp_effort(&self, effort: Effort) -> Effort {
        let supported = self.supported_efforts();
        if supported.contains(&effort) {
            return effort;
        }
        supported
            .iter()
            .find(|candidate| **candidate > effort)
            .or_else(|| {
                supported
                    .iter()
                    .rev()
                    .find(|candidate| **candidate < effort)
            })
            .copied()
            .unwrap_or(Effort::Off)
    }
}

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
    /// C8: set when the tool's input is one raw text argument under a
    /// grammar. openai-responses sends it as a custom tool (no JSON escaping
    /// tax); every other adapter ignores this and keeps the JSON schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freeform: Option<FreeformFormat>,
}

/// A freeform tool's wire grammar. The dialect is Lark, the one syntax the
/// adapters emit, so the definition travels alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreeformFormat {
    pub definition: String,
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
