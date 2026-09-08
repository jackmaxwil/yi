use serde::{Deserialize, Serialize};

/// Design §12 model roles. Each role names a `provider/id`; an unset role falls back to the
/// primary, so a config that names nothing behaves as one model for everything.
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

/// X7: `~/.yi/config.json`, the whole user surface. Config is not durable state, so §19 rule
/// 4 does not apply: an unknown key is a typo to name, not a field to preserve.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserConfig {
    pub model: Option<String>,
    pub thinking: Option<crate::model::Effort>,
    pub models: Option<ModelRoles>,
    pub bash: Option<BashConfig>,
    pub plan: Option<PlanConfig>,
    pub plans: Option<PlansConfig>,
    pub mcp: Option<McpConfig>,
    pub kernel: Option<KernelConfig>,
    pub edit: Option<EditConfig>,
    pub keys: Option<std::collections::BTreeMap<String, String>>,
    pub console: Option<ConsoleConfig>,
    pub tui: Option<TuiConfig>,
    pub lanes: Option<crate::lane::LanesConfig>,
    pub catalog: Option<CatalogConfig>,
    pub telemetry: Option<TelemetryConfig>,
    /// `routing`: OpenRouter's `provider` object, sent verbatim; absent means
    /// `{"sort": "throughput"}`, and `{}` restores OpenRouter's load balancing.
    pub routing: Option<serde_json::Value>,
}

/// `tui.pace`: the streamed reveal's speed as a percentage of the default (100); `0` paints
/// text the instant it arrives, as before the reveal existed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TuiConfig {
    pub pace: Option<u16>,
}

/// `telemetry.enabled`: write one span per request, tool call, turn and compaction to
/// `<session file>.telemetry.jsonl`; off by default, on for every CI run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelemetryConfig {
    pub enabled: Option<bool>,
}

/// `catalog.refreshHours`: how old `~/.yi/catalog/<provider>.json` may be before a session
/// refreshes it in the background (24); `catalog.enabled: false` never fetches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogConfig {
    pub enabled: Option<bool>,
    pub refresh_hours: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConsoleConfig {
    pub auto_side: Option<bool>,
}

/// `kernel.prewarm`: boot the IPython kernel in the background at session open so the first
/// cell pays execution only. Default on; `false` keeps the boot lazy.
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

/// `plans.dir`, default `.yi/plans` relative to the workspace root. X7's project layer is
/// unbuilt, so it reads from the user's own config and is global to every workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlansConfig {
    pub dir: Option<String>,
}

/// D36: MCP is compiled in but runtime-gated; `mcp.enabled` is the switch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpConfig {
    pub enabled: Option<bool>,
    pub token_store: Option<String>,
}
