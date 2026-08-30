use serde::{Deserialize, Serialize};

use serde_json::Value;

use crate::message::{AgentMessage, Content, StopReason};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantMessageEvent {
    Start {
        partial: AgentMessage,
    },
    TextStart {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        partial: AgentMessage,
    },
    TextDelta {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        delta: String,
        partial: AgentMessage,
    },
    TextEnd {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        content: String,
        partial: AgentMessage,
    },
    ThinkingStart {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        partial: AgentMessage,
    },
    ThinkingDelta {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        delta: String,
        partial: AgentMessage,
    },
    ThinkingEnd {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        content: String,
        partial: AgentMessage,
    },
    #[serde(rename = "toolcall_start")]
    ToolCallStart {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        partial: AgentMessage,
    },
    #[serde(rename = "toolcall_delta")]
    ToolCallDelta {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        delta: String,
        partial: AgentMessage,
    },
    #[serde(rename = "toolcall_end")]
    ToolCallEnd {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        #[serde(rename = "toolCall")]
        tool_call: Content,
        partial: AgentMessage,
    },
    Done {
        reason: StopReason,
        message: AgentMessage,
    },
    Error {
        reason: StopReason,
        error: AgentMessage,
    },
}

/// The closed failure taxonomy in a tool result's `details.errorKind`, which
/// `yi stats` aggregates; the wire name is the variant in lower snake case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolErrorKind {
    Denied,
    NotFound,
    InvalidArgs,
    Aborted,
    StaleTag,
    NoopLoop,
    ToolError,
}

impl ToolErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::NotFound => "not_found",
            Self::InvalidArgs => "invalid_args",
            Self::Aborted => "aborted",
            Self::StaleTag => "stale_tag",
            Self::NoopLoop => "noop_loop",
            Self::ToolError => "tool_error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResult {
    pub content: Vec<Content>,
    pub details: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<crate::message::Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_tool_names: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminate: Option<bool>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "wire DTOs mirror the JSON; variants are parse-then-drop, boxing buys nothing"
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    AgentStart,
    AgentEnd {
        messages: Vec<AgentMessage>,
    },
    TurnStart,
    TurnEnd {
        message: AgentMessage,
        #[serde(rename = "toolResults")]
        tool_results: Vec<AgentMessage>,
    },
    MessageStart {
        message: AgentMessage,
    },
    MessageUpdate {
        message: AgentMessage,
        #[serde(rename = "assistantMessageEvent")]
        assistant_message_event: AssistantMessageEvent,
    },
    MessageEnd {
        message: AgentMessage,
    },
    ToolExecutionStart {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        args: Value,
    },
    ToolExecutionUpdate {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        args: Value,
        #[serde(rename = "partialResult")]
        partial_result: ToolResult,
    },
    ToolExecutionEnd {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        result: ToolResult,
        #[serde(rename = "isError")]
        is_error: bool,
    },
    PermissionRequested {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        title: String,
        description: String,
    },
    PermissionResolved {
        #[serde(rename = "toolCallId")]
        tool_call_id: String,
        allowed: bool,
    },
    ChildUpdate {
        update: crate::subagent::ChildUpdate,
    },
}
