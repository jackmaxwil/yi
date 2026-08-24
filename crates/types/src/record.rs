use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::message::{AgentMessage, StopReason, Usage};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum OperationIntent {
    #[serde(rename_all = "camelCase")]
    Run {
        original_prompt: Vec<AgentMessage>,
        initial_messages: Vec<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        system_prompt_override: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        resume_data: Option<Map<String, Value>>,
    },
    #[serde(rename_all = "camelCase")]
    Compaction {
        #[serde(skip_serializing_if = "Option::is_none")]
        custom_instructions: Option<String>,
        result_entry_id: String,
    },
    #[serde(rename_all = "camelCase")]
    Navigation {
        target_id: Option<String>,
        summarize: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        custom_instructions: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        summary_entry_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LaneRecord {
    #[serde(rename_all = "camelCase")]
    OperationStarted {
        id: String,
        lane: String,
        source_leaf_id: Option<String>,
        intent: OperationIntent,
        seq: u64,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    AbortRequested {
        id: String,
        lane: String,
        run_id: String,
        seq: u64,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    OperationFinished {
        id: String,
        lane: String,
        run_id: String,
        outcome: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<Value>,
        seq: u64,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    StepAttempt {
        id: String,
        lane: String,
        run_id: String,
        step: String,
        attempt: u64,
        result_entry_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        compaction_reason: Option<String>,
        seq: u64,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    ToolStarted {
        id: String,
        lane: String,
        run_id: String,
        assistant_entry_id: String,
        tool_index: u64,
        tool_call_id: String,
        tool_name: String,
        effective_args: Map<String, Value>,
        result_entry_id: String,
        replay: String,
        seq: u64,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    QueueEnqueued {
        id: String,
        lane: String,
        queue: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        target: Value,
        seq: u64,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    QueueCancelled {
        id: String,
        lane: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        entry_id: String,
        seq: u64,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    WriteDeferred {
        id: String,
        lane: String,
        run_id: String,
        target: Value,
        seq: u64,
        timestamp: u64,
    },
    #[serde(rename_all = "camelCase")]
    Usage {
        id: String,
        lane: String,
        usage: Usage,
        cause: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        entry_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        attempt: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stop_reason: Option<StopReason>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Value>,
        seq: u64,
        timestamp: u64,
    },
}

impl LaneRecord {
    pub fn id(&self) -> &str {
        match self {
            Self::OperationStarted { id, .. }
            | Self::AbortRequested { id, .. }
            | Self::OperationFinished { id, .. }
            | Self::StepAttempt { id, .. }
            | Self::ToolStarted { id, .. }
            | Self::QueueEnqueued { id, .. }
            | Self::QueueCancelled { id, .. }
            | Self::WriteDeferred { id, .. }
            | Self::Usage { id, .. } => id,
        }
    }

    pub fn lane(&self) -> &str {
        match self {
            Self::OperationStarted { lane, .. }
            | Self::AbortRequested { lane, .. }
            | Self::OperationFinished { lane, .. }
            | Self::StepAttempt { lane, .. }
            | Self::ToolStarted { lane, .. }
            | Self::QueueEnqueued { lane, .. }
            | Self::QueueCancelled { lane, .. }
            | Self::WriteDeferred { lane, .. }
            | Self::Usage { lane, .. } => lane,
        }
    }

    pub fn seq(&self) -> u64 {
        match self {
            Self::OperationStarted { seq, .. }
            | Self::AbortRequested { seq, .. }
            | Self::OperationFinished { seq, .. }
            | Self::StepAttempt { seq, .. }
            | Self::ToolStarted { seq, .. }
            | Self::QueueEnqueued { seq, .. }
            | Self::QueueCancelled { seq, .. }
            | Self::WriteDeferred { seq, .. }
            | Self::Usage { seq, .. } => *seq,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::OperationStarted { .. } => "operation_started",
            Self::AbortRequested { .. } => "abort_requested",
            Self::OperationFinished { .. } => "operation_finished",
            Self::StepAttempt { .. } => "step_attempt",
            Self::ToolStarted { .. } => "tool_started",
            Self::QueueEnqueued { .. } => "queue_enqueued",
            Self::QueueCancelled { .. } => "queue_cancelled",
            Self::WriteDeferred { .. } => "write_deferred",
            Self::Usage { .. } => "usage",
        }
    }

    pub fn run_id(&self) -> Option<&str> {
        match self {
            Self::OperationStarted { .. } => None,
            Self::AbortRequested { run_id, .. }
            | Self::OperationFinished { run_id, .. }
            | Self::StepAttempt { run_id, .. }
            | Self::ToolStarted { run_id, .. }
            | Self::WriteDeferred { run_id, .. } => Some(run_id),
            Self::QueueEnqueued { run_id, .. }
            | Self::QueueCancelled { run_id, .. }
            | Self::Usage { run_id, .. } => run_id.as_deref(),
        }
    }

    pub fn operation_kind(&self) -> Option<&'static str> {
        match self {
            Self::OperationStarted { intent, .. } => Some(match intent {
                OperationIntent::Run { .. } => "run",
                OperationIntent::Compaction { .. } => "compaction",
                OperationIntent::Navigation { .. } => "navigation",
            }),
            _ => None,
        }
    }

    pub fn usage(&self) -> Option<&Usage> {
        match self {
            Self::Usage { usage, .. } => Some(usage),
            _ => None,
        }
    }

    pub fn assign(&mut self, new_seq: u64, new_timestamp: u64) {
        match self {
            Self::OperationStarted { seq, timestamp, .. }
            | Self::AbortRequested { seq, timestamp, .. }
            | Self::OperationFinished { seq, timestamp, .. }
            | Self::StepAttempt { seq, timestamp, .. }
            | Self::ToolStarted { seq, timestamp, .. }
            | Self::QueueEnqueued { seq, timestamp, .. }
            | Self::QueueCancelled { seq, timestamp, .. }
            | Self::WriteDeferred { seq, timestamp, .. }
            | Self::Usage { seq, timestamp, .. } => {
                *seq = new_seq;
                *timestamp = new_timestamp;
            }
        }
    }
}
