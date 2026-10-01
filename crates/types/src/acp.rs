use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC 2.0 method frame, either direction (request when `id` is
/// present, notification otherwise).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpFrame {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 success response to an ACP client request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpResponse {
    pub jsonrpc: String,
    pub id: Value,
    pub result: Value,
}

/// JSON-RPC 2.0 error response to an ACP client request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpErrorResponse {
    pub jsonrpc: String,
    pub id: Value,
    pub error: AcpErrorShape,
}

/// JSON-RPC error object; `code` follows the JSON-RPC 2.0 vocabulary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpErrorShape {
    pub code: i64,
    pub message: String,
}

/// JSON-RPC 2.0 notification (agent → client), e.g. `session/update`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpNotification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Value,
}

/// Params of the `session/update` notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpUpdateParams {
    pub session_id: String,
    pub update: AcpSessionUpdate,
}

/// ACP v2 `session/update` payload — the subset of update kinds Yi emits
/// plus an extension carrier for `_yi/*` updates (design §17.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub enum AcpSessionUpdate {
    #[serde(rename_all = "camelCase")]
    AgentMessageChunk {
        message_id: String,
        content: AcpContentBlock,
    },
    #[serde(rename_all = "camelCase")]
    AgentThoughtChunk {
        message_id: String,
        content: AcpContentBlock,
    },
    #[serde(rename_all = "camelCase")]
    AgentMessage {
        message_id: String,
        content: Vec<AcpContentBlock>,
    },
    #[serde(rename_all = "camelCase")]
    UserMessage {
        message_id: String,
        content: Vec<AcpContentBlock>,
    },
    StateUpdate(AcpState),
    ToolCallUpdate(AcpToolCallUpdate),
    #[serde(rename_all = "camelCase")]
    ToolCallContentChunk {
        tool_call_id: String,
        content: AcpToolContent,
    },
    #[serde(rename_all = "camelCase")]
    TerminalUpdate {
        terminal_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        command: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        exit_status: Option<AcpTerminalExit>,
    },
    #[serde(rename_all = "camelCase")]
    TerminalOutputChunk {
        terminal_id: String,
        data: String,
    },
    #[serde(rename_all = "camelCase")]
    UsageUpdate {
        used: u64,
        size: u64,
    },
    /// Extension or future update kinds; `sessionUpdate` values beginning
    /// with `_` are implementation extensions (`_yi/*`, design §17.2).
    #[serde(untagged)]
    Extension(AcpExtensionUpdate),
}

/// ACP v2 tool-call upsert: omitted fields leave the client's value unchanged; also the
/// `toolCall` of a permission subject.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpToolCallUpdate {
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<AcpToolKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AcpToolCallStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<AcpToolContent>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_output: Option<Value>,
}

/// The daemon's session ledger on disk, keyed by session id: what a
/// `session/list` without a cwd answers from after a restart.
#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonLedger {
    pub sessions: BTreeMap<String, DaemonLedgerEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonLedgerEntry {
    pub cwd: String,
    pub unseen: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_state: Option<String>,
    pub last_event_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// Carrier for `_yi/*` extension updates and unknown future update kinds;
/// the discriminator plus its fields re-emit verbatim (§20).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpExtensionUpdate {
    #[serde(rename = "sessionUpdate")]
    pub session_update: String,
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

/// ACP v2 session state (`state_update`); `stopReason` accompanies `idle`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AcpState {
    Running,
    #[serde(rename_all = "camelCase")]
    Idle {
        #[serde(skip_serializing_if = "Option::is_none")]
        stop_reason: Option<AcpStopReason>,
    },
    RequiresAction,
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

/// ACP content block — Yi emits text; unknown blocks round-trip via `Other`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcpContentBlock {
    Text {
        text: String,
    },
    #[serde(untagged)]
    Other(AcpOtherBlock),
}

/// Unknown content block preserved verbatim (§20: unknown tags re-emit).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpOtherBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

/// ACP tool-call content: a content block, a display-only terminal reference, or a diff;
/// unknown types round-trip via `Other`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcpToolContent {
    Content {
        content: AcpContentBlock,
    },
    #[serde(rename_all = "camelCase")]
    Terminal {
        terminal_id: String,
    },
    /// `changes` is authoritative for the paths; `patch` renders some or all of them.
    Diff {
        changes: Vec<AcpDiffChange>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        patch: Option<AcpDiffPatch>,
    },
    #[serde(untagged)]
    Other(AcpOtherBlock),
}

/// One file a diff touches; `path` is absolute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpDiffChange {
    pub operation: AcpDiffOperation,
    pub path: String,
}

/// ACP v2 diff operations Yi emits; the rest round-trip via `Other`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpDiffOperation {
    Add,
    Modify,
    #[serde(untagged)]
    Other(String),
}

/// Renderable patch text; the unified diff is absolute-pathed so `git apply` takes it verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpDiffPatch {
    pub format: AcpPatchFormat,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpPatchFormat {
    GitPatch,
    #[serde(untagged)]
    Other(String),
}

/// ACP v2 tool-call status vocabulary; unknown values round-trip via `Other`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    Cancelled,
    #[serde(untagged)]
    Other(String),
}

/// ACP v2 tool kind vocabulary; unknown values round-trip via `Unknown`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
    #[serde(untagged)]
    Unknown(String),
}

/// Exit information for a terminal update; the wire's `exitCode` is unsigned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpTerminalExit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u32>,
}

/// `initialize` response body; `protocolVersion` is always 2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpInitializeResult {
    pub protocol_version: u16,
    pub info: AcpImplementation,
    pub capabilities: Value,
    pub auth_methods: Vec<Value>,
}

/// Implementation info advertised in `initialize`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpImplementation {
    pub name: String,
    pub version: String,
}

/// `session/new` and `session/resume` response body; resume carries no `sessionId`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub config_options: Vec<AcpConfigOption>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

/// The spec forbids custom fields at the root of its types; Yi's ride `_meta.yi`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AcpMeta {
    pub yi: AcpYiMeta,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpYiMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The entry offset a resume replayed up to, for the next resume's `replayFrom`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replayed_to: Option<u64>,
}

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

/// `session/prompt` response: sent once the prompt is inserted, naming its `user_message`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPromptResult {
    pub message_id: String,
}

/// `session/request_permission` request params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPermissionParams {
    pub session_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub options: Vec<AcpPermissionOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<AcpPermissionSubject>,
}

/// What an ask is about; a diff under approval rides as the tool call's content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcpPermissionSubject {
    #[serde(rename_all = "camelCase")]
    ToolCall { tool_call: Box<AcpToolCallUpdate> },
    #[serde(untagged)]
    Other(AcpOtherBlock),
}

/// One selectable permission option.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: AcpPermissionOptionKind,
}

/// ACP v2 permission option kinds; unknown values round-trip via `Other`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpPermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    #[serde(untagged)]
    Other(String),
}

/// `session/request_permission` response outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum AcpPermissionOutcome {
    Cancelled,
    #[serde(rename_all = "camelCase")]
    Selected {
        option_id: String,
    },
}
