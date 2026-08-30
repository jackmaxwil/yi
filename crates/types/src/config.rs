use serde::{Deserialize, Serialize};

/// Design §12 model roles. Each role names a `provider/id`; an unset role
/// falls back to the primary model, so a config that names nothing behaves
/// exactly as one model for everything.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelRoles {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summarizer: Option<String>,
    /// Naming this role is what turns the LLM reviewer on (D28/D50: the
    /// advisor observes and says nothing until a model role names it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advisor: Option<String>,
    /// Naming this role turns the M7 auto reviewer on. Unset, auto mode is the
    /// deterministic ladder and nothing extra is ever constructed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_review: Option<String>,
}

/// X7: `~/.yi/config.json`, the whole user surface. Config is not durable
/// state, so §19 rule 4 does not apply — an unknown key is a typo the user
/// wants named, not a field to preserve.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserConfig {
    pub model: Option<String>,
    pub thinking: Option<crate::model::Effort>,
    pub models: Option<ModelRoles>,
    pub bash: Option<BashConfig>,
    pub plan: Option<PlanConfig>,
    pub mcp: Option<McpConfig>,
    pub kernel: Option<KernelConfig>,
    pub edit: Option<EditConfig>,
    pub keys: Option<std::collections::BTreeMap<String, String>>,
}

/// `kernel.prewarm`: boot the IPython kernel in the background at session
/// open so the first cell pays execution only. Default on; `false` keeps
/// the boot lazy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelConfig {
    pub prewarm: Option<bool>,
}

/// `edit.freeformGrammar`: send the patch language as a provider grammar
/// rather than a JSON argument. Off until a live round trip confirms it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditConfig {
    pub freeform_grammar: Option<bool>,
}

/// `bash.autoBackgroundMs`, off unless the user sets it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BashConfig {
    pub auto_background_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanConfig {
    pub stale_reminder_turns: Option<u64>,
}

/// D36: MCP is compiled in but runtime-gated; `mcp.enabled` is the switch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpConfig {
    pub enabled: Option<bool>,
    pub token_store: Option<String>,
}
