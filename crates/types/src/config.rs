use serde::{Deserialize, Serialize};

/// Design §5 model roles. Each role names a `provider/id`. An unset summarizer falls back to the
/// primary; the advisor, auto reviewer and classifier do nothing until a role names them.
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
    /// Naming a checkpoint (`english`) turns on the local `classifier` sidecar; unset, nothing
    /// is constructed. It is never a chat model, so it never falls back to the primary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classifier: Option<String>,
    /// Naming a model turns on line selection: after the reducer cuts a bash output, it picks
    /// the line ranges of the full output that explain it. Unset, nothing is asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    /// Search's separate OpenRouter completion; unset uses Yi's built-in search model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web_search: Option<String>,
}

/// `~/.yi/config.json` is the whole user surface. Config is not durable state, so §20's
/// unknown-data rule does not apply: an unknown key is a typo to name, not a field to preserve.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserConfig {
    pub model: Option<String>,
    pub thinking: Option<crate::model::Effort>,
    pub models: Option<ModelRoles>,
    pub web_search: Option<WebSearchConfig>,
    pub bash: Option<BashConfig>,
    pub plan: Option<PlanConfig>,
    pub plans: Option<PlansConfig>,
    pub mcp: Option<McpConfig>,
    pub kernel: Option<KernelConfig>,
    pub edit: Option<EditConfig>,
    pub keys: Option<std::collections::BTreeMap<String, String>>,
    pub console: Option<ConsoleConfig>,
    pub tui: Option<TuiConfig>,
    /// `theme.dark` and `theme.light` name a file in `~/.yi/themes`. Unset, the built-in palette stays.
    pub theme: Option<ThemeConfig>,
    pub lanes: Option<crate::lane::LanesConfig>,
    pub catalog: Option<CatalogConfig>,
    pub telemetry: Option<TelemetryConfig>,
    /// `routing`: OpenRouter's `provider` object, sent verbatim; absent, no object is sent and
    /// OpenRouter routes by its own ranking. A schema request adds `require_parameters`.
    pub routing: Option<serde_json::Value>,
    /// `rlm.maxDepth`, how deep a family may nest (default 1, ceiling 3) (D165).
    pub rlm: Option<RlmConfig>,
    pub spend: Option<SpendConfig>,
    pub compaction: Option<CompactionConfig>,
    pub node: Option<NodeConfig>,
    pub skills: Option<SkillsConfig>,
    pub classifier: Option<ClassifierConfig>,
    pub permissions: Option<PermissionsConfig>,
    pub sandbox: Option<SandboxConfig>,
    pub mail: Option<MailConfig>,
    /// `memory.importClaude`: import the Claude Code memories of this working directory and
    /// the user-level one into the stores at session start; on unless set false.
    pub memory: Option<MemoryConfig>,
    /// `open`: the apps a console message's Open in submenu lists, each a name and a command
    /// whose `{path}` argument is the message saved as markdown; absent, the submenu is too.
    pub open: Option<Vec<OpenApp>>,
    pub browser: Option<BrowserConfig>,
    /// `providers`: OpenAI-compatible endpoints the user declares, each addressed `<id>/<model>`.
    pub providers: Option<Providers>,
}

/// Ids a custom provider may not take beside the catalog's: the classifier's bearer, the script.
const RESERVED_PROVIDERS: [&str; 2] = ["laya", "faux"];

/// Invariant: no id is a built-in provider's, empty, or holds `/`, which splits `provider/id`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(try_from = "std::collections::BTreeMap<String, CustomProvider>")]
pub struct Providers(std::collections::BTreeMap<String, CustomProvider>);

impl TryFrom<std::collections::BTreeMap<String, CustomProvider>> for Providers {
    type Error = String;

    fn try_from(map: std::collections::BTreeMap<String, CustomProvider>) -> Result<Self, String> {
        let builtin = crate::CATALOG_PROVIDERS.iter().chain(&RESERVED_PROVIDERS);
        if let Some(taken) = map
            .keys()
            .find(|id| builtin.clone().any(|known| known == id))
        {
            return Err(format!(
                "providers: `{taken}` is a built-in provider id; name the custom one otherwise"
            ));
        }
        if let Some(bad) = map.keys().find(|id| id.is_empty() || id.contains('/')) {
            return Err(format!(
                "providers: `{bad}` is not a provider id (empty, or holds `/`)"
            ));
        }
        for (id, provider) in &map {
            let url = &provider.base_url;
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(format!(
                    "providers.{id}.baseUrl: `{url}` is not an http(s) URL"
                ));
            }
            let mut seen = std::collections::BTreeSet::new();
            if provider.models.is_empty() {
                return Err(format!("providers.{id}.models: declares no model"));
            }
            if let Some(bad) = (provider.models.iter())
                .find(|model| !seen.insert(&model.id) || model.id.is_empty())
            {
                return Err(format!(
                    "providers.{id}.models: `{}` is empty or declared twice",
                    bad.id
                ));
            }
        }
        Ok(Self(map))
    }
}

impl Providers {
    pub fn iter(&self) -> impl Iterator<Item = (&str, &CustomProvider)> {
        self.0.iter().map(|(id, provider)| (id.as_str(), provider))
    }
}

/// Speaks OpenAI chat completions; the key is `apiKeyEnv`'s variable, else the literal `apiKey`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomProvider {
    pub base_url: String,
    pub api_key_env: Option<String>,
    pub api_key: Option<String>,
    pub models: Vec<CustomModel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomModel {
    pub id: String,
    pub name: Option<String>,
    pub reasoning: Option<bool>,
    pub input: Option<Vec<CustomInput>>,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
    pub cost: Option<CustomCost>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomInput {
    Text,
    Image,
}

/// Dollars per million tokens; an unset rate is 0.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CustomCost {
    pub input: Option<serde_json::Number>,
    pub output: Option<serde_json::Number>,
    pub cache_read: Option<serde_json::Number>,
    pub cache_write: Option<serde_json::Number>,
}

impl CustomProvider {
    /// The catalog entries this provider declares under `id`.
    pub fn catalog(&self, id: &str) -> Vec<crate::model::Model> {
        let rate = |set: Option<&serde_json::Number>| set.cloned().unwrap_or_else(|| 0u64.into());
        let base_url = self.base_url.trim_end_matches('/').to_owned();
        (self.models.iter())
            .map(|model| {
                let cost = model.cost.clone().unwrap_or_default();
                let input = model.input.as_deref().unwrap_or(&[CustomInput::Text]);
                crate::model::Model {
                    id: model.id.clone(),
                    name: model.name.clone().unwrap_or_else(|| model.id.clone()),
                    api: "openai-completions".to_owned(),
                    provider: id.to_owned(),
                    base_url: base_url.clone(),
                    reasoning: model.reasoning.unwrap_or(false),
                    input: (input.iter())
                        .map(|kind| match kind {
                            CustomInput::Text => "text".to_owned(),
                            CustomInput::Image => "image".to_owned(),
                        })
                        .collect(),
                    cost: crate::model::ModelCost {
                        input: rate(cost.input.as_ref()),
                        output: rate(cost.output.as_ref()),
                        cache_read: rate(cost.cache_read.as_ref()),
                        cache_write: rate(cost.cache_write.as_ref()),
                        tiers: None,
                    },
                    context_window: model.context_window.unwrap_or(128_000),
                    max_tokens: model.max_tokens.unwrap_or(16_384),
                    compat: Some(serde_json::json!({"plainMaxTokens": true})),
                    thinking_level_map: None,
                    headers: None,
                    read_timeout_secs: None,
                }
            })
            .collect()
    }
}

/// `memory.importClaude`: off stops the start-of-session import and nothing else; the
/// `yi memory import-claude` verb still runs it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryConfig {
    pub import_claude: Option<bool>,
}

/// `webSearch.engine`: the OpenRouter server-search engine; unset is `exa`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebSearchConfig {
    pub engine: Option<String>,
}

/// `mail.hopLimit`: the longest chain of letters between sessions, each sent by a turn another
/// letter woke, before a send is refused; unset is 20.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MailConfig {
    pub hop_limit: Option<u32>,
}

/// `browser.path`: the Chrome or Chromium the console's browser pane runs; unset, the installed
/// one is found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserConfig {
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpenApp {
    pub name: String,
    pub command: Vec<String>,
}

/// `permissions.mode`: the mode a run starts in when no `--auto`, `--confirm` or `--yolo` is
/// given; unset is `auto`. `yi setup` writes it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionsConfig {
    pub mode: Option<ModeName>,
    /// Directories contained commands and write tools may write besides the workspace.
    pub allow_write: Option<Vec<String>>,
    /// Calls that run unasked, each `TOOL PATTERN`: `bash cargo test*`. A refusal names its rule.
    pub allow: Option<Vec<String>>,
}

/// `egress`: hosts a contained command reaches unasked, beside the seed list. Read from the home
/// config alone, so a tree cannot name its own exfiltration host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SandboxConfig {
    pub egress: Option<Vec<crate::egress::Domain>>,
    pub host_recipes: Option<Vec<HostRecipe>>,
}

/// `call`: words a command starts with, run outside the sandbox unasked while every `pin` (a path
/// in the tree) matches the trunk on origin. Read from the home config alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostRecipe {
    pub call: String,
    pub pin: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeName {
    Auto,
    Ask,
    Yolo,
}

/// `classifier`: the endpoint's URL, one decision's deadline, and the confidence it points at
/// (0.9 when unset). The bearer is the OpenRouter key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClassifierConfig {
    pub url: Option<String>,
    pub timeout_ms: Option<u64>,
    pub threshold: Option<serde_json::Number>,
    /// Before `approval` existed: `false` is `wait-for-user`, `true` is `instant`.
    pub approve: Option<bool>,
    /// When the classifier answers an auto-mode approval; unset is `instant`.
    pub approval: Option<ApprovalMode>,
    pub allow_at: Option<serde_json::Number>,
    pub allow_destructive_at: Option<serde_json::Number>,
    pub ask_at: Option<serde_json::Number>,
    /// The `after-delay` wait before the classifier answers an ask nobody has; 0 never hands it over.
    pub ask_timeout_secs: Option<u64>,
    /// The score at which outside text is marked as addressing the agent: 0.8 when unset.
    pub screen_at: Option<serde_json::Number>,
    /// The head of a longer outside text the screen reads: 96 000 chars when unset.
    pub screen_max_chars: Option<u64>,
}

/// `instant` asks the classifier before the person; `after-delay` once the person has not
/// answered for `askTimeoutSecs`; `wait-for-user` never.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalMode {
    Instant,
    AfterDelay,
    WaitForUser,
}

/// `node`: overrides `~/.yi/node.json` field by field; `slots` bounds the kernels this
/// machine holds live at once, and `isolation` the placements a spawn may ask for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeConfig {
    pub slots: Option<std::num::NonZeroU8>,
    pub isolation: Option<Vec<String>>,
}

/// `spend.alertTokens`, `spend.alertUsd` (a session and its children) and `spend.dayAlertUsd`
/// (the local day, every session): a notice per multiple crossed; absent, no alert.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpendConfig {
    pub alert_tokens: Option<u64>,
    pub alert_usd: Option<serde_json::Number>,
    pub day_alert_usd: Option<serde_json::Number>,
}

/// `compaction.at`: compact once a request passes this many tokens, when that comes before the
/// window's reserve; absent or 0, only the reserve decides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompactionConfig {
    pub at: Option<u64>,
}

/// Keys an older Yi read that this one does not, each with why it went: D182 deleted the
/// artifact and closure stop gates that `gates` switched.
const REMOVED: &[(&str, &str)] = &[("gates", "the artifact and closure stop gates are gone")];

/// A key [`migrate`] dropped before the strict parse, and why, so a config that loaded
/// yesterday still loads and the caller names each drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigMigration {
    key: &'static str,
    why: &'static str,
}

impl std::fmt::Display for ConfigMigration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Self { key, why } = self;
        write!(f, "the config's `{key}` key is ignored: {why}; delete it")
    }
}

/// Rewrites a raw config into the shape [`UserConfig`] parses; a current config comes back
/// unchanged with no migrations, so running it twice is running it once.
pub fn migrate(config: &mut serde_json::Value) -> Vec<ConfigMigration> {
    let Some(keys) = config.as_object_mut() else {
        return Vec::new();
    };
    REMOVED
        .iter()
        .filter(|(key, _)| keys.remove(*key).is_some())
        .map(|&(key, why)| ConfigMigration { key, why })
        .collect()
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

/// `tui.follow`: `slide` keeps one-row following, `page` jumps half a pane, and `still`
/// holds the view while text arrives. Unset resolves to `page` unless macOS asks for `still`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FollowMode {
    Slide,
    #[default]
    Page,
    Still,
}

/// `tui.pace`: the streamed reveal's speed as a percentage of the default (100); `0` paints
/// text the instant it arrives, as before the reveal existed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TuiConfig {
    pub pace: Option<u16>,
    pub follow: Option<FollowMode>,
}

/// `theme.dark` / `theme.light`: the omp theme file `~/.yi/themes/<name>.json` for that ground.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThemeConfig {
    pub dark: Option<String>,
    pub light: Option<String>,
}

/// `telemetry.enabled`: write one span per request, tool call, turn and compaction to
/// `<session file>.telemetry.jsonl`; off by default, on for every CI run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelemetryConfig {
    pub enabled: Option<bool>,
}

/// `skills.global`: the skills under the home roots (`~/.yi`, `~/.agents`, `~/.pi`, `~/.claude`)
/// that the root session's catalog lists; absent, it lists only the repository's own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillsConfig {
    pub global: Option<Vec<String>>,
}

/// `catalog.refreshHours`: how old `~/.yi/catalog/<provider>.json` may be before a session
/// refreshes it in the background (24); `catalog.enabled: false` never fetches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogConfig {
    pub enabled: Option<bool>,
    pub refresh_hours: Option<u64>,
}

/// `console.titleIntro`: draw the title field at console start (on); `console.titleWeights`: scene
/// name to weight; `console.sidebar`: the state the console starts in, which ⌥b writes as it cycles.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConsoleConfig {
    pub auto_side: Option<bool>,
    pub title_intro: Option<bool>,
    pub title_weights: Option<std::collections::BTreeMap<String, u32>>,
    pub sidebar: Option<SidebarState>,
    /// Project directories in the order the sidebar lists them; one with no sessions is ignored.
    pub project_order: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarState {
    Rail,
    Full,
    Hidden,
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

/// `bash.autoBackgroundMs`: unset is 60 s, or off for a run that exits once idle; `0` is off.
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
