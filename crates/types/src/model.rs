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

    /// Invariant: every `#[serde(alias)]` spelling on [`Effort`] is accepted here too; the
    /// parsers are separate, and a split means a file that loads but a flag that errors.
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
    /// Invariant: the advertised levels, low to high, never empty — an all-`null` model falls
    /// back to [`Effort::Off`], so callers take `.first()`/`.last()` with no empty case.
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
    /// C8: set when the tool's input is one raw text argument under a grammar.
    /// openai-responses sends it as a custom tool; every other adapter keeps the JSON schema.
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

/// Whether a later request reads this request's tail, said by the caller (D295). A missing
/// value means `Loop`, the side whose failure is one spare write rather than a total miss.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reuse {
    /// The next request continues this conversation: its tail is read again.
    #[default]
    Loop,
    /// A single call (a title, a compaction, a branch summary): the tail is never read again.
    OneShot,
    /// The conversation's last turn (`tool_choice: none`): nothing follows it.
    LastTurn,
}

impl Reuse {
    pub fn is_loop(&self) -> bool {
        *self == Self::Loop
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmContext {
    pub system_prompt: String,
    pub messages: Vec<crate::message::AgentMessage>,
    /// Set by the caller, never inferred from the request's shape.
    #[serde(default, skip_serializing_if = "Reuse::is_loop")]
    pub reuse: Reuse,
    /// Per-request facts (the environment block), rendered after every cache mark and
    /// never carrying one: such an entry is written every request and read by none (D295).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transient: Vec<crate::message::AgentMessage>,
    /// A structured-output schema. Providers render it ahead of tools and system, so a
    /// prefix key hashes it first; read by structured outputs, unread by the adapters yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
}

/// The assistant diagnostic that records [`LlmContext::stable_key`] per request (C5).
pub const CACHE_DIAGNOSTIC: &str = "cache";

impl LlmContext {
    /// A hash of the inputs every request of a conversation shares: schema, tools, system.
    /// A change between two requests is a miss yi caused (design §7, invariant 3).
    pub fn stable_key(&self) -> String {
        use sha2::{Digest, Sha256};
        let rendered = serde_json::json!([self.schema, self.tools, self.system_prompt]);
        let digest = Sha256::digest(rendered.to_string().as_bytes());
        digest
            .iter()
            .take(8)
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
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
    fn transient_and_schema_ride_only_when_set() -> Result<(), Box<dyn std::error::Error>> {
        let context = LlmContext {
            system_prompt: "s".to_owned(),
            messages: Vec::new(),
            transient: vec![crate::message::AgentMessage::host_user(
                crate::message::UserContent::Text("<environment>".to_owned()),
                0,
            )],
            schema: Some(serde_json::json!({"type": "object"})),
            reuse: super::Reuse::OneShot,
            tools: None,
            tool_choice: None,
        };
        let wire = serde_json::to_string(&context)?;
        assert!(wire.contains(r#""transient":[{"#), "{wire}");
        assert!(wire.contains(r#""schema":{"type":"object"}"#), "{wire}");
        assert!(wire.contains(r#""reuse":"one_shot""#), "{wire}");
        assert_eq!(serde_json::from_str::<LlmContext>(&wire)?, context);
        let older: LlmContext = serde_json::from_str(r#"{"systemPrompt":"s","messages":[]}"#)?;
        assert_eq!(
            older.reuse,
            super::Reuse::Loop,
            "a missing value is the loop"
        );
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

    /// The key moves with each input it covers, and not with the history or the tail.
    #[test]
    fn the_stable_key_moves_with_schema_tools_and_system_only() {
        let base = LlmContext {
            system_prompt: "s".to_owned(),
            messages: Vec::new(),
            transient: Vec::new(),
            schema: None,
            reuse: super::Reuse::Loop,
            tools: Some(vec![super::ToolDef {
                name: "read".to_owned(),
                description: "d".to_owned(),
                parameters: serde_json::json!({}),
                freeform: None,
            }]),
            tool_choice: None,
        };
        let key = base.stable_key();
        let mut system = base.clone();
        system.system_prompt.push('!');
        let mut tools = base.clone();
        if let Some(tool) = tools.tools.iter_mut().flatten().next() {
            tool.description.push('!');
        }
        let mut schema = base.clone();
        schema.schema = Some(serde_json::json!({"type": "object"}));
        for moved in [system, tools, schema] {
            assert_ne!(moved.stable_key(), key);
        }
        let mut history = base.clone();
        history
            .transient
            .push(crate::message::AgentMessage::host_user(
                crate::message::UserContent::Text("<environment>".to_owned()),
                0,
            ));
        assert_eq!(history.stable_key(), key);
    }
}
