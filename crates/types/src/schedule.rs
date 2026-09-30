use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::plan::doc::TodoLabel;
use crate::url::Url;

/// Job lifecycle (design §15.2); each status serializes as its lowercase wire string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Active,
    Paused,
    Completed,
    Cancelled,
    /// A state a newer Yi wrote; it re-emits verbatim.
    #[serde(untagged)]
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobSource {
    Cron,
    Heartbeat,
    RlmHeartbeat,
}

/// How a due prompt reaches a busy session (design §15.2): `Steer` interrupts
/// the current turn at the next boundary, `FollowUp` waits for idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    Steer,
    FollowUp,
}

/// Schedule shape (design §15.2). `expression` keeps the user's original text;
/// `interval_ms` is set only for `Interval`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScheduleKind {
    Once,
    Cron,
    Interval,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronSchedule {
    pub kind: ScheduleKind,
    pub expression: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u64>,
}

/// What a tick does while an earlier tick's todo is open: `skip` creates none, `buffer_one`
/// keeps at most one more waiting behind it, `allow` always creates. Absent is `skip`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Overlap {
    Skip,
    BufferOne,
    Allow,
    #[serde(untagged)]
    Other(String),
}

/// Ticks that came due while the host slept or was down: `once` creates one todo whose note
/// counts them, `skip` creates none, `all` creates one per tick. Absent is `once`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatchUp {
    Once,
    Skip,
    All,
    #[serde(untagged)]
    Other(String),
}

/// A clock subscription `clock://<schedule>` (§15.2, D39: epoch ms, Yi's own format): a tick
/// makes a todo of `label`, `prompt` (its note) and `intent`, or unblocks `unblocks`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub status: JobStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<JobSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_mode: Option<DeliveryMode>,
    pub session_id: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub prompt: String,
    pub schedule: CronSchedule,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_skipped_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    pub run_count: u64,
    /// The addresses of the user's words that set the subscription up.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intent: Vec<Url>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlap: Option<Overlap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catch_up: Option<CatchUp>,
    /// A durable timer: the session todo blocked on this job's `clock://` address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unblocks: Option<TodoLabel>,
    /// Held by the kill switch, apart from `status`, until `/heartbeat resume`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub halted: bool,
    /// A channel subscription: each tick reads the buffer past its ack instead of the clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<crate::channel::ChannelSub>,
    /// The plan owning the todo `unblocks` names, which unblocks through that plan's engine
    /// whether or not the session's list shows the row: a sub-plan's is never shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<crate::plan::doc::PlanId>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The key a tick's stamp rides under in the todo it creates.
pub const CLOCK_KEY: &str = "clock";

/// A tick's stamp on its todo: the job that ticked, the tick's time, and how many ticks the
/// todo stands for, more than one when ticks were missed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClockStamp {
    pub job: String,
    pub at: u64,
    pub ticks: u64,
}

pub const HALT_ENTRY_TYPE: &str = "halt";

/// The `custom{halt}` entry: the kill switch (`halted`) or its undo, with the clock jobs it held
/// or released and the sessions whose running turn it interrupted or whose hold it lifted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HaltRecord {
    pub halted: bool,
    pub at: u64,
    pub jobs: u64,
    pub sessions: u64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A claimed-but-unresolved delivery (design §15.2): persisted before the prompt
/// is delivered so a crash between claim and delivery is recoverable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchRecord {
    pub id: String,
    pub job_id: String,
    pub claimed_at: u64,
    pub scheduled_for: u64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The `scheduled-jobs.json` disk shape.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleState {
    #[serde(default)]
    pub jobs: Vec<Job>,
    #[serde(default)]
    pub dispatches: Vec<DispatchRecord>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl crate::entry::CustomRecord for HaltRecord {
    const TYPE: &'static str = HALT_ENTRY_TYPE;
}
