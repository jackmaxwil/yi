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
    #[serde(alias = "med")]
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

    /// Invariant: every `#[serde(alias)]` spelling on [`Effort`] is accepted
    /// here too — the two parsers are separate, and a spelling the wire takes
    /// and this one refuses is a plan file that loads but a flag that errors.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "med" {
            return Ok(Self::Medium);
        }
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

/// Schema fact: every provider validates a tool name against
/// `^[a-zA-Z0-9_-]{1,128}$`, so a name outside it is a wire rejection.
pub const TOOL_NAME_MAX: usize = 128;

/// The tool a turn is forced onto. Checked at construction, so no adapter can
/// be handed a name it is unable to put on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ForcedTool(String);

impl ForcedTool {
    pub fn new(name: &str) -> Result<Self, ToolChoiceError> {
        let Some(offending) = name.chars().find(|character| {
            !character.is_ascii_alphanumeric() && *character != '_' && *character != '-'
        }) else {
            return match name.len() {
                0 => Err(ToolChoiceError::EmptyName),
                length if length > TOOL_NAME_MAX => Err(ToolChoiceError::NameTooLong {
                    name: name.to_owned(),
                    length,
                    max: TOOL_NAME_MAX,
                }),
                _ => Ok(Self(name.to_owned())),
            };
        };
        Err(ToolChoiceError::NameCharacter {
            name: name.to_owned(),
            offending,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ForcedTool {
    type Error = ToolChoiceError;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        Self::new(&name)
    }
}

impl From<ForcedTool> for String {
    fn from(tool: ForcedTool) -> Self {
        tool.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolChoiceError {
    EmptyName,
    NameTooLong {
        name: String,
        length: usize,
        max: usize,
    },
    NameCharacter {
        name: String,
        offending: char,
    },
}

impl std::fmt::Display for ToolChoiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyName => write!(formatter, "forced tool name is empty"),
            Self::NameTooLong { name, length, max } => write!(
                formatter,
                "forced tool name {name:?} is {length} characters, over the {max} cap"
            ),
            Self::NameCharacter { name, offending } => write!(
                formatter,
                "forced tool name {name:?} carries {offending:?}, outside [A-Za-z0-9_-]"
            ),
        }
    }
}

impl std::error::Error for ToolChoiceError {}

/// A turn's tool posture. The three provider adapters spell these same three
/// intents in three different wire shapes, so the choice travels typed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    None,
    Tool(ForcedTool),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmContext {
    pub system_prompt: String,
    pub messages: Vec<crate::message::AgentMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
}

#[cfg(test)]
mod tests {
    use super::{Effort, ForcedTool, LlmContext, TOOL_NAME_MAX, ToolChoiceError};
    use std::str::FromStr;

    #[test]
    fn a_forced_tool_name_is_checked_against_the_provider_pattern() {
        assert_eq!(ForcedTool::new(""), Err(ToolChoiceError::EmptyName));
        assert!(matches!(
            ForcedTool::new("plan op"),
            Err(ToolChoiceError::NameCharacter { offending: ' ', .. })
        ));
        assert!(matches!(
            ForcedTool::new(&"p".repeat(TOOL_NAME_MAX + 1)),
            Err(ToolChoiceError::NameTooLong {
                length: 129,
                max: 128,
                ..
            })
        ));
        assert_eq!(
            ForcedTool::new("plan_op-1").map(|tool| tool.as_str().to_owned()),
            Ok("plan_op-1".to_owned())
        );
    }

    #[test]
    fn a_context_without_a_choice_keeps_its_bytes() -> Result<(), Box<dyn std::error::Error>> {
        let wire = r#"{"systemPrompt":"s","messages":[]}"#;
        let context: LlmContext = serde_json::from_str(wire)?;
        assert_eq!(context.tool_choice, None);
        assert_eq!(serde_json::to_string(&context)?, wire);
        Ok(())
    }

    #[test]
    fn med_parses_on_both_paths_and_never_serializes() -> Result<(), Box<dyn std::error::Error>> {
        for spelling in ["med", "medium"] {
            assert_eq!(Effort::from_str(spelling), Ok(Effort::Medium));
            let wire: Effort = serde_json::from_str(&format!("\"{spelling}\""))?;
            assert_eq!(wire, Effort::Medium);
        }
        assert_eq!(serde_json::to_string(&Effort::Medium)?, "\"medium\"");
        Ok(())
    }
}
