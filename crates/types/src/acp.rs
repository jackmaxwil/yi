use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The fields a relay routes a frame by, read in place; the frame itself travels as its bytes.
/// A field spelled with an escape fails the read, and the caller parses the whole frame.
#[derive(Debug, Default, Deserialize)]
pub struct AcpRoute<'a> {
    #[serde(borrow, default)]
    pub method: Option<&'a str>,
    /// Presence only, a null included, since presence is what makes a request.
    #[serde(default, deserialize_with = "present")]
    pub id: bool,
    #[serde(borrow, default)]
    pub params: Option<AcpRouteParams<'a>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRouteParams<'a> {
    #[serde(borrow, default)]
    pub session_id: Option<&'a str>,
    #[serde(borrow, default)]
    pub update: Option<AcpRouteUpdate<'a>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpRouteUpdate<'a> {
    #[serde(borrow, default)]
    pub session_update: Option<&'a str>,
}

fn present<'de, D: serde::Deserializer<'de>>(value: D) -> Result<bool, D::Error> {
    serde::de::IgnoredAny::deserialize(value).map(|_| true)
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
    /// When the session was pinned to the top of the console's sidebar, which lists pins in
    /// this order; absent means unpinned. Not `pinned`: the v1 fixture keeps that as an unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_ms: Option<u64>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
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
    /// What the person typed in the entry a `before` fork left out, to ask again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefill: Option<String>,
    /// The child that raised a `session/request_permission`; absent for the root session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asker: Option<AcpAsker>,
    /// Present when the session has a rewriter model: the length its composer control starts at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewriter: Option<crate::config::RewriteLength>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpAsker {
    pub child_id: String,
}

/// `_yi/pending` result: the permission requests still waiting for an answer, each in the
/// shape it was sent in and answerable by its original `requestId`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpPendingResult {
    pub requests: Vec<AcpPendingRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPendingRequest {
    pub request_id: String,
    pub session_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub options: Vec<AcpPermissionOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<AcpPermissionSubject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<u64>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
}

/// `_yi/fork` params: a new session branched from `entryId`, leaving it out (`before`) or
/// keeping it (`at`); the source session is untouched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpForkParams {
    pub session_id: String,
    pub entry_id: String,
    #[serde(default)]
    pub position: AcpForkPosition,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpForkPosition {
    #[default]
    Before,
    At,
    #[serde(untagged)]
    Other(String),
}

/// `_yi/committed`: a user or assistant message with intervening active-main entries, so a
/// client can connect a live message through ancestors that are not visible in its transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpCommitted {
    pub entry: crate::entry::Entry,
    /// Ancestors after the prior commit cursor, oldest first;
    /// [`AcpCommitted::entry`] is the suffix's final item.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ancestors: Vec<crate::entry::Entry>,
    /// Cut row after a capped ancestry suffix, naming the replay limits and the full-branch call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ancestors_notice: Option<String>,
}

/// `session/prompt` response: sent once the prompt is inserted, or at once with `queue_place` when queued mid-turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpPromptResult {
    pub message_id: String,
    /// Present when the prompt arrived mid-turn and was queued; 1 is next after the running turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_place: Option<u64>,
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
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<AcpMeta>,
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

/// The agent's request that its client show a thing; only a client that set `_meta.yi.shows`
/// when it opened the session is sent one.
pub const SHOW_METHOD: &str = "_yi/show";

/// `_yi/show` request params: the agent asks its client to put one thing in front of the user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpShowParams {
    pub session_id: String,
    /// A path, `path:LINE` or a `yi://` address, as the model wrote it; the client resolves it.
    pub address: String,
    /// The session's working directory: what a relative path in `address` is relative to.
    pub cwd: String,
    pub strength: AcpShowStrength,
}

/// How hard a `_yi/show` asks; `front` may still land as a tab when the user is busy there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpShowStrength {
    Peek,
    Tab,
    Front,
    #[serde(untagged)]
    Other(String),
}

/// `_yi/show` response: what the client did, and its own words on why when it has any.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpShowResult {
    pub outcome: AcpShowOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What became of a `_yi/show`; `refused` is a thing the client cannot show.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcpShowOutcome {
    Peek,
    Tab,
    Front,
    Refused,
    #[serde(untagged)]
    Other(String),
}
