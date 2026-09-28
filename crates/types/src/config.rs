use serde::{Deserialize, Serialize};

/// Design §5 model roles. Each role names a `provider/id`; an unset role falls back to the
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
    /// Naming this role turns the §8 auto reviewer on. Unset, auto mode is the
    /// deterministic ladder and nothing extra is ever constructed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_review: Option<String>,
}

/// `~/.yi/config.json` is the whole user surface. Config is not durable state, so §20's
/// unknown-data rule does not apply: an unknown key is a typo to name, not a field to preserve.
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
    /// `routing`: OpenRouter's `provider` object, sent verbatim; absent deprioritises
    /// upstreams under 20 tok/s or over 10 s p50 latency, and `{}` sends nothing.
    pub routing: Option<serde_json::Value>,
    /// `rlm.maxDepth`, how deep a family may nest (default 1, ceiling 3) (D165).
    pub rlm: Option<RlmConfig>,
    pub spend: Option<SpendConfig>,
    pub node: Option<NodeConfig>,
}

/// `node`: overrides `~/.yi/node.json` field by field; `slots` bounds the kernels this
/// machine holds live at once, and `isolation` the placements a spawn may ask for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeConfig {
    pub slots: Option<std::num::NonZeroU8>,
    pub isolation: Option<Vec<String>>,
}

/// `spend.alertTokens`: a notice each time a session's tokens, its children's included, cross
/// another multiple of it; absent, no alert.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpendConfig {
    pub alert_tokens: Option<u64>,
}

/// A key an older Yi read that this one does not: [`migrate`] drops it before the strict
/// parse, so a config that loaded yesterday still loads, and the caller names each drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigMigration {
    /// `gates` switched the artifact and closure stop gates, which D182 deleted.
    RemovedGates,
}

impl std::fmt::Display for ConfigMigration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RemovedGates => f.write_str(
                "the config's `gates` key is ignored: the artifact and closure stop gates are gone; delete it",
            ),
        }
    }
}

/// Rewrites a raw config into the shape [`UserConfig`] parses; a current config comes back
/// unchanged with no migrations, so running it twice is running it once.
pub fn migrate(config: &mut serde_json::Value) -> Vec<ConfigMigration> {
    let mut applied = Vec::new();
    if let Some(keys) = config.as_object_mut()
        && keys.remove("gates").is_some()
    {
        applied.push(ConfigMigration::RemovedGates);
    }
    applied
}

/// The one config load, with the migrations it took. A current config skips `Value`, which
/// reads a doubled key last-wins and drops the error's line and column.
pub fn parse(raw: &str) -> Result<(UserConfig, Vec<ConfigMigration>), serde_json::Error> {
    let strict = match serde_json::from_str(raw) {
        Ok(config) => return Ok((config, Vec::new())),
        Err(error) => error,
    };
    let mut config = serde_json::from_str(raw)?;
    let applied = migrate(&mut config);
    if applied.is_empty() {
        return Err(strict);
    }
    Ok((serde_json::from_value(config)?, applied))
}

/// The nesting depth a family gets with no `rlm.maxDepth` set (D165's floor of the 1..=3 range).
pub const DEFAULT_MAX_DEPTH: u8 = 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RlmConfig {
    pub max_depth: Option<u8>,
}

impl RlmConfig {
    /// The nesting depth a family may reach: the configured value clamped to 1..=3.
    pub fn depth(&self) -> u8 {
        self.max_depth.unwrap_or(DEFAULT_MAX_DEPTH).clamp(1, 3)
    }
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

/// `plans.dir`, default `.yi/plans` relative to the workspace root. §17.1's project layer is
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
