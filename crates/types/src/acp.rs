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
/// plus an extension carrier for `_yi/*` updates (design C3/C9).
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
    #[serde(rename_all = "camelCase")]
    ToolCallUpdate {
        tool_call_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        kind: Option<AcpToolKind>,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<AcpToolCallStatus>,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<Vec<AcpToolContent>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw_input: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw_output: Option<Value>,
    },
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
    /// with `_` are implementation extensions (`_yi/*`, design C9).
    #[serde(untagged)]
    Extension(AcpExtensionUpdate),
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
/// the discriminator plus its fields re-emit verbatim (§19).
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

/// Unknown content block preserved verbatim (§19: unknown tags re-emit).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpOtherBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

/// ACP tool-call content: a content block, or a display-only terminal
/// reference (design C7; diff content lands with T13).
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
    /// Design C7: `changes` are the paths the call would touch, `patch` the
    /// T13 unified diff, absolute-pathed so `git apply` accepts it verbatim.
    Diff {
        changes: Vec<String>,
        patch: String,
    },
}

/// ACP v2 tool-call status vocabulary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// ACP v2 tool kind vocabulary (subset Yi maps to).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpToolKind {
    Read,
    Edit,
    Search,
    Execute,
    Fetch,
    Other,
}

/// Exit information for a terminal update.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpTerminalExit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
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

/// `session/new` and `session/resume` response body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionResult {
    pub session_id: String,
    pub config_options: Vec<AcpConfigOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpConfigOption {
    pub config_id: String,
    pub name: String,
    pub kind: Value,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<AcpToolContent>>,
}

/// One selectable permission option.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: AcpPermissionOptionKind,
}

/// ACP v2 permission option kinds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpPermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
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
