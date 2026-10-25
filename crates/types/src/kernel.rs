use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Jupyter connection file (design §9). Field order is the byte order of the
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

/// Jupyter wire message header (design §9). Field order follows the Jupyter
/// messaging spec, and serde writes fields in declaration order.
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
/// and signature (design §9).
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

/// One media attachment captured from an attachment `display_data` payload
/// (design §9). Snake_case by design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KernelAttachment {
    pub mime_type: String,
    pub data: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Python error from an `error` iopub message (design §9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KernelError {
    pub ename: String,
    pub evalue: String,
    pub traceback: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecuteStatus {
    Ok,
    Error,
    Aborted,
    /// A state a newer Yi wrote; it re-emits verbatim.
    #[serde(untagged)]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteResult {
    pub stdout: String,
    pub stderr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<KernelAttachment>,
    pub status: ExecuteStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<KernelError>,
    pub duration_ms: u64,
}

/// One tool a cell called through `tool.call`, listed on that cell's `ipython` result (design
/// §9.2): the files a write named, a command it ran, a person's reply; never the content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedCall {
    pub tool: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    pub duration_ms: u64,
    /// Set on a call that finished after the cell that started it closed: it lands on a later
    /// cell's result, and the todo landed count does not credit it to that cell's turn.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub outlived: bool,
}

/// The `ipython` result's `details` key holding its cell's [`NestedCall`] rows.
pub const NESTED_CALLS: &str = "calls";
/// The `details` key counting read rows a cell made past [`NESTED_READS_KEPT`].
pub const NESTED_READS_DROPPED: &str = "callsDropped";
/// Read rows one cell keeps; every other row is kept, since a row holds paths, never content.
pub const NESTED_READS_KEPT: usize = 256;

/// `.bootstrap-version` marker (design §9.1): any mismatch forces a full venv
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
/// reason (design §9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KernelSnapshotSkip {
    pub name: String,
    #[serde(default)]
    pub reason: String,
}

/// Marker-line result of one namespace snapshot (design §9). The kernel-side
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

/// Marker-line result of one namespace restore (design §9).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct KernelRestoreResult {
    #[serde(default)]
    pub restored: Vec<String>,
    #[serde(default)]
    pub failed: Vec<KernelSnapshotSkip>,
    #[serde(default)]
    pub path: String,
}
