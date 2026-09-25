use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Job lifecycle (design §15.2); each status serializes as its lowercase wire string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Active,
    Paused,
    Completed,
    Cancelled,
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

/// One scheduled job (design §15.2); timestamps are epoch milliseconds and Yi
/// owns this file format (D39).
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
