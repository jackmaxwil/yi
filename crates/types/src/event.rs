use serde::{Deserialize, Serialize};

use serde_json::Value;

use crate::message::{AgentMessage, Content, StopReason};

/// A delta is a delta (D145): only `Start` carries the message; every other
/// event carries its own bytes, and [`apply`] folds them into one accumulator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantMessageEvent {
    Start {
        partial: AgentMessage,
    },
    TextStart {
        #[serde(rename = "contentIndex")]
        content_index: usize,
    },
    TextDelta {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        delta: String,
    },
    TextEnd {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        content: String,
    },
    ThinkingStart {
        #[serde(rename = "contentIndex")]
        content_index: usize,
    },
    ThinkingDelta {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        delta: String,
    },
    ThinkingEnd {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        content: String,
    },
    #[serde(rename = "toolcall_start")]
    ToolCallStart {
        #[serde(rename = "contentIndex")]
        content_index: usize,
    },
    #[serde(rename = "toolcall_delta")]
    ToolCallDelta {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        delta: String,
    },
    #[serde(rename = "toolcall_end")]
    ToolCallEnd {
        #[serde(rename = "contentIndex")]
        content_index: usize,
        #[serde(rename = "toolCall")]
        tool_call: Content,
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

/// Folds one event into the message in place: `*Start` pushes a block, deltas grow it,
/// `*End` settles it, and a whole message (`Start`, `Done`, `Error`) replaces it.
pub fn apply(message: &mut AgentMessage, event: &AssistantMessageEvent) {
    let content = match message {
        AgentMessage::Assistant { content, .. } => content,
        _ => match event {
            AssistantMessageEvent::Start { partial } => {
                *message = partial.clone();
                return;
            }
            _ => return,
        },
    };
    match event {
        AssistantMessageEvent::Start { partial } => *message = partial.clone(),
        AssistantMessageEvent::Done { message: done, .. } => *message = done.clone(),
        AssistantMessageEvent::Error { error, .. } => *message = error.clone(),
        AssistantMessageEvent::TextStart { content_index } => place(
            content,
            *content_index,
            Content::Text {
                text: String::new(),
                text_signature: None,
            },
        ),
        AssistantMessageEvent::ThinkingStart { content_index } => place(
            content,
            *content_index,
            Content::Thinking {
                thinking: String::new(),
                thinking_signature: None,
                redacted: None,
            },
        ),
        AssistantMessageEvent::ToolCallStart { content_index } => place(
            content,
            *content_index,
            Content::ToolCall {
                id: String::new(),
                name: String::new(),
                arguments: serde_json::Map::new(),
                thought_signature: None,
                namespace: None,
            },
        ),
        AssistantMessageEvent::TextDelta {
            content_index,
            delta,
        } => {
            if let Some(Content::Text { text, .. }) = content.get_mut(*content_index) {
                text.push_str(delta);
            }
        }
        AssistantMessageEvent::ThinkingDelta {
            content_index,
            delta,
        } => {
            if let Some(Content::Thinking { thinking, .. }) = content.get_mut(*content_index) {
                thinking.push_str(delta);
            }
        }
        AssistantMessageEvent::TextEnd {
            content_index,
            content: full,
        } => {
            if let Some(Content::Text { text, .. }) = content.get_mut(*content_index) {
                *text = full.clone();
            }
        }
        AssistantMessageEvent::ThinkingEnd {
            content_index,
            content: full,
        } => {
            if let Some(Content::Thinking { thinking, .. }) = content.get_mut(*content_index) {
                *thinking = full.clone();
            }
        }
        AssistantMessageEvent::ToolCallEnd {
            content_index,
            tool_call,
        } => place(content, *content_index, tool_call.clone()),
        AssistantMessageEvent::ToolCallDelta { .. } => {}
    }
}

/// Puts `block` at `index`, growing the vector so an out-of-order start
/// (a provider that numbers blocks past the ones it sent) still lands.
fn place(content: &mut Vec<Content>, index: usize, block: Content) {
    if index < content.len() {
        if let Some(slot) = content.get_mut(index) {
            *slot = block;
        }
        return;
    }
    while content.len() < index {
        content.push(Content::Text {
            text: String::new(),
            text_signature: None,
        });
    }
    content.push(block);
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
    LandingState {
        landing: crate::lane::Landing,
    },
}
