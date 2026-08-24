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
