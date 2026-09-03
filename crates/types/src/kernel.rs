use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Jupyter connection file (design K2). Field order is the byte order of the
/// file the host writes; ipykernel re-writes it with resolved ports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub ip: String,
    pub transport: String,
    pub shell_port: u32,
    pub iopub_port: u32,
    pub stdin_port: u32,
    pub control_port: u32,
    pub hb_port: u32,
    pub signature_scheme: String,
    pub key: String,
    #[serde(default = "default_kernel_name")]
    pub kernel_name: String,
}

fn default_kernel_name() -> String {
    "python3".to_owned()
}

/// Jupyter wire message header (design K3). Field order is the reference
/// `buildMessage` serialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JupyterHeader {
    pub msg_id: String,
    pub session: String,
    pub username: String,
    pub date: String,
    pub msg_type: String,
    pub version: String,
}

/// One Jupyter message: the four JSON frames after the `<IDS|MSG>` delimiter
/// and signature (design K3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JupyterMessage {
    pub header: JupyterHeader,
    #[serde(default)]
    pub parent_header: Map<String, Value>,
    #[serde(default)]
    pub metadata: Map<String, Value>,
    #[serde(default)]
    pub content: Map<String, Value>,
}

/// One file edit captured from a diff `display_data` payload (design K6).
/// Wire casing is snake_case by design (§6): the Python `edit` skill emits it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KernelDiffDisplay {
    pub path: String,
    pub old_str: String,
    pub new_str: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u64>,
}

/// One media attachment captured from an attachment `display_data` payload
/// (design K6). Snake_case by design (§6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KernelAttachment {
    pub mime_type: String,
    pub data: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Agent-message receipt echoed back through `display_data` (design K6).
/// CamelCase by design (§6): the host's own receipt, echoed verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KernelSentAgentMessage {
    pub id: String,
    pub message: String,
    pub delivery_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receiver_role: Option<String>,
    pub target: KernelAgentMessageTarget,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KernelAgentMessageTarget {
    pub active_session_id: String,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
}

/// Python error from an `error` iopub message (design K6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KernelError {
    pub ename: String,
    pub evalue: String,
    pub traceback: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecuteStatus {
    Ok,
    Error,
    Aborted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteResult {
    pub stdout: String,
    pub stderr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diffs: Vec<KernelDiffDisplay>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<KernelAttachment>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sent_agent_messages: Vec<KernelSentAgentMessage>,
    pub status: ExecuteStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<KernelError>,
    pub duration_ms: u64,
}

/// `.bootstrap-version` marker (design K1): any mismatch forces a full venv
/// rebuild.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapVersion {
    pub schema: u64,
    pub ipykernel: String,
    pub runtime: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One name the snapshot/restore helpers could not process, with a short
/// reason (design K10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KernelSnapshotSkip {
    pub name: String,
    #[serde(default)]
    pub reason: String,
}

/// Marker-line result of one namespace snapshot (design K10). The kernel-side
/// helper prints it as a single JSON line after the result marker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KernelSnapshotResult {
    #[serde(default)]
    pub saved: Vec<String>,
    #[serde(default)]
    pub skipped: Vec<KernelSnapshotSkip>,
    /// Oversized live variables removed by an explicit compaction prune.
    #[serde(default)]
    pub pruned: Vec<String>,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub path: String,
}

/// Marker-line result of one namespace restore (design K10).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct KernelRestoreResult {
    #[serde(default)]
    pub restored: Vec<String>,
    #[serde(default)]
    pub failed: Vec<KernelSnapshotSkip>,
    #[serde(default)]
    pub path: String,
}
