use serde_json::Value;
use yi_types::acp::{
    AcpContentBlock, AcpExtensionUpdate, AcpSessionUpdate, AcpState, AcpStopReason,
    AcpTerminalExit, AcpToolCallStatus, AcpToolContent, AcpToolKind,
};
use yi_types::entry::Entry;
use yi_types::event::{AgentEvent, AssistantMessageEvent};
use yi_types::goal::Goal;
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
use yi_types::subagent::ChildId;

pub fn extension<K: Into<String>>(
    kind: &str,
    fields: impl IntoIterator<Item = (K, Value)>,
) -> AcpSessionUpdate {
    AcpSessionUpdate::Extension(AcpExtensionUpdate {
        session_update: kind.to_owned(),
        fields: fields
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    })
}

/// The runtime event verbatim: what a yi client reduces with solo's own reducer.
pub fn event_update(event: &AgentEvent, seq: u64, child: Option<&ChildId>) -> AcpSessionUpdate {
    let mut fields = vec![
        ("event", serde_json::to_value(event).unwrap_or(Value::Null)),
        ("seq", Value::from(seq)),
    ];
    if let Some(child) = child {
        fields.push(("childId", Value::String(child.0.clone())));
    }
    extension("_yi/event", fields)
}

pub fn gap_update(seq: u64, dropped: u64, child: Option<&ChildId>) -> AcpSessionUpdate {
    let mut fields = vec![("seq", Value::from(seq)), ("dropped", Value::from(dropped))];
    if let Some(child) = child {
        fields.push(("childId", Value::String(child.0.clone())));
    }
    extension("_yi/event_gap", fields)
}

pub struct ReplayFrame<'a> {
    pub entries: &'a [Entry],
    pub from: u64,
    pub replayed_to: u64,
    pub leaf: Option<&'a str>,
    pub name: Option<&'a str>,
    pub goal: Option<&'a Goal>,
    pub todos: Option<&'a yi_types::todo::TodoList>,
    pub context_window: u64,
    pub child: Option<&'a ChildId>,
}

pub fn replay_update(frame: &ReplayFrame<'_>) -> AcpSessionUpdate {
    let mut fields = vec![
        (
            "entries",
            serde_json::to_value(frame.entries).unwrap_or(Value::Null),
        ),
        ("from", Value::from(frame.from)),
        ("replayedTo", Value::from(frame.replayed_to)),
        (
            "leafId",
            frame
                .leaf
                .map_or(Value::Null, |leaf| Value::String(leaf.to_owned())),
        ),
        (
            "name",
            frame
                .name
                .map_or(Value::Null, |name| Value::String(name.to_owned())),
        ),
        (
            "todos",
            frame
                .todos
                .and_then(|todos| serde_json::to_value(todos).ok())
                .unwrap_or(Value::Null),
        ),
        (
            "goal",
            frame
                .goal
                .and_then(|goal| serde_json::to_value(goal).ok())
                .unwrap_or(Value::Null),
        ),
        ("contextWindow", Value::from(frame.context_window)),
    ];
    if let Some(child) = frame.child {
        fields.push(("childId", Value::String(child.0.clone())));
    }
    extension("_yi/replay", fields)
}

/// Per-session translation state (design C3): message ids are allocated
/// here, tool-call and terminal ids pass through from the event stream.
#[derive(Debug, Default)]
pub struct IdMap {
    next_message: u64,
    current_message: Option<String>,
    last_stop: Option<AcpStopReason>,
    context_window: u64,
}

impl IdMap {
    pub fn new(context_window: u64) -> Self {
        Self {
            context_window,
            ..Self::default()
        }
    }

    fn allocate(&mut self) -> String {
        self.next_message = self.next_message.saturating_add(1);
        let id = format!("msg_{}", self.next_message);
        self.current_message = Some(id.clone());
        id
    }

    fn current(&mut self) -> String {
        match &self.current_message {
            Some(id) => id.clone(),
            None => self.allocate(),
        }
    }
}

fn stop_reason(reason: StopReason) -> AcpStopReason {
    match reason {
        StopReason::Stop | StopReason::ToolUse | StopReason::Pending | StopReason::Deferred => {
            AcpStopReason::EndTurn
        }
        StopReason::Length => AcpStopReason::MaxTokens,
        StopReason::Aborted => AcpStopReason::Cancelled,
        StopReason::Error => AcpStopReason::Other("error".to_owned()),
    }
}

fn tool_kind(tool_name: &str) -> AcpToolKind {
    match tool_name {
        "read" | "glob" => AcpToolKind::Read,
        "edit" | "write" => AcpToolKind::Edit,
        "grep" => AcpToolKind::Search,
        "bash" | "ipython" => AcpToolKind::Execute,
        _ => AcpToolKind::Other,
    }
}

fn text_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn user_blocks(content: &UserContent) -> Vec<AcpContentBlock> {
    match content {
        UserContent::Text(text) => vec![AcpContentBlock::Text { text: text.clone() }],
        UserContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(|block| match block {
                Content::Text { text, .. } => Some(AcpContentBlock::Text { text: text.clone() }),
                _ => None,
            })
            .collect(),
    }
}

/// Wraps a Custom message as a `_yi/<custom_type>` extension update (C9).
fn extension_of(
    custom_type: &str,
    content: &UserContent,
    details: Option<&Value>,
) -> AcpSessionUpdate {
    let text = match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => text_of(blocks),
    };
    let mut fields = vec![("text", Value::String(text))];
    if let Some(details) = details {
        fields.push(("details", details.clone()));
    }
    extension(&format!("_yi/{custom_type}"), fields)
}

/// Standard base64 for terminal output chunks; std has no encoder and a
/// dependency for 15 lines fails the ladder.
pub fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let quad = [
            TABLE[(n >> 18) as usize & 63],
            TABLE[(n >> 12) as usize & 63],
            TABLE[(n >> 6) as usize & 63],
            TABLE[n as usize & 63],
        ];
        let keep = chunk.len().saturating_add(1);
        for (index, byte) in quad.iter().enumerate() {
            out.push(if index < keep { char::from(*byte) } else { '=' });
        }
    }
    out
}

fn tool_call_update(
    tool_call_id: &str,
    status: AcpToolCallStatus,
    raw_output: Option<Value>,
    content: Option<Vec<AcpToolContent>>,
) -> AcpSessionUpdate {
    AcpSessionUpdate::ToolCallUpdate {
        tool_call_id: tool_call_id.to_owned(),
        title: None,
        kind: None,
        status: Some(status),
        content,
        raw_input: None,
        raw_output,
    }
}

/// Pure event → update mapping (design C3, C7 terminal half, C9).
/// Exhaustive over `AgentEvent` so a new variant fails compile, not wire.
pub fn to_updates(event: &AgentEvent, ids: &mut IdMap) -> Vec<AcpSessionUpdate> {
    match event {
        AgentEvent::AgentStart => vec![AcpSessionUpdate::StateUpdate(AcpState::Running)],
        AgentEvent::AgentEnd { .. } => vec![AcpSessionUpdate::StateUpdate(AcpState::Idle {
            stop_reason: Some(ids.last_stop.take().unwrap_or(AcpStopReason::EndTurn)),
        })],
        AgentEvent::TurnStart | AgentEvent::TurnEnd { .. } => Vec::new(),
        AgentEvent::MessageStart { message } => match message {
            AgentMessage::Assistant { .. } => {
                ids.allocate();
                Vec::new()
            }
            AgentMessage::User { content, .. } => vec![AcpSessionUpdate::UserMessage {
                message_id: ids.allocate(),
                content: user_blocks(content),
            }],
            AgentMessage::Custom {
                custom_type,
                content,
                details,
                ..
            } => vec![extension_of(custom_type, content, details.as_ref())],
            _ => Vec::new(),
        },
        AgentEvent::MessageUpdate {
            assistant_message_event,
            ..
        } => match assistant_message_event {
            AssistantMessageEvent::TextDelta { delta, .. } => {
                vec![AcpSessionUpdate::AgentMessageChunk {
                    message_id: ids.current(),
                    content: AcpContentBlock::Text {
                        text: delta.clone(),
                    },
                }]
            }
            AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                vec![AcpSessionUpdate::AgentThoughtChunk {
                    message_id: ids.current(),
                    content: AcpContentBlock::Text {
                        text: delta.clone(),
                    },
                }]
            }
            AssistantMessageEvent::Done { reason, .. }
            | AssistantMessageEvent::Error { reason, .. } => {
                ids.last_stop = Some(stop_reason(*reason));
                Vec::new()
            }
            _ => Vec::new(),
        },
        AgentEvent::MessageEnd { message } => {
            ids.current_message = None;
            match message {
                AgentMessage::Assistant { usage, .. } => vec![AcpSessionUpdate::UsageUpdate {
                    used: u64::try_from(usage.total_tokens).unwrap_or(0),
                    size: ids.context_window,
                }],
                _ => Vec::new(),
            }
        }
        AgentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => vec![AcpSessionUpdate::ToolCallUpdate {
            tool_call_id: tool_call_id.clone(),
            title: Some(tool_name.clone()),
            kind: Some(tool_kind(tool_name)),
            status: Some(AcpToolCallStatus::InProgress),
            content: None,
            raw_input: Some(args.clone()),
            raw_output: None,
        }],
        AgentEvent::ToolExecutionUpdate {
            tool_call_id,
            partial_result,
            ..
        } => vec![AcpSessionUpdate::ToolCallContentChunk {
            tool_call_id: tool_call_id.clone(),
            content: AcpToolContent::Content {
                content: AcpContentBlock::Text {
                    text: text_of(&partial_result.content),
                },
            },
        }],
        AgentEvent::ToolExecutionEnd {
            tool_call_id,
            tool_name,
            result,
            is_error,
        } => {
            let status = if *is_error {
                AcpToolCallStatus::Failed
            } else {
                AcpToolCallStatus::Completed
            };
            let text = text_of(&result.content);
            if tool_name == "bash" {
                let exit_code = result.details.get("exitCode").and_then(Value::as_i64);
                vec![
                    AcpSessionUpdate::TerminalOutputChunk {
                        terminal_id: tool_call_id.clone(),
                        data: base64(text.as_bytes()),
                    },
                    AcpSessionUpdate::TerminalUpdate {
                        terminal_id: tool_call_id.clone(),
                        command: None,
                        exit_status: Some(AcpTerminalExit { exit_code }),
                    },
                    tool_call_update(
                        tool_call_id,
                        status,
                        None,
                        Some(vec![AcpToolContent::Terminal {
                            terminal_id: tool_call_id.clone(),
                        }]),
                    ),
                ]
            } else {
                vec![tool_call_update(
                    tool_call_id,
                    status,
                    Some(result.details.clone()),
                    Some(vec![AcpToolContent::Content {
                        content: AcpContentBlock::Text { text },
                    }]),
                )]
            }
        }
        AgentEvent::PermissionRequested { .. } => {
            vec![AcpSessionUpdate::StateUpdate(AcpState::RequiresAction)]
        }
        AgentEvent::PermissionResolved { .. } => {
            vec![AcpSessionUpdate::StateUpdate(AcpState::Running)]
        }
        AgentEvent::ChildUpdate { update } => {
            object_extension("_yi/subagent_update", serde_json::to_value(update))
        }
        AgentEvent::LandingState { landing } => {
            object_extension("_yi/landing", serde_json::to_value(landing))
        }
    }
}

fn object_extension(name: &str, value: serde_json::Result<Value>) -> Vec<AcpSessionUpdate> {
    let fields = value
        .ok()
        .and_then(|value| value.as_object().cloned())
        .map(|map| map.into_iter().collect())
        .unwrap_or_default();
    vec![AcpSessionUpdate::Extension(AcpExtensionUpdate {
        session_update: name.to_owned(),
        fields,
    })]
}

/// Replay (design C6): a stored branch walked into full-message updates.
pub fn replay_updates(entries: &[Entry], ids: &mut IdMap) -> Vec<AcpSessionUpdate> {
    let mut updates = Vec::new();
    for entry in entries {
        match entry {
            Entry::Message { message, .. } => match message {
                AgentMessage::Assistant { content, .. } => {
                    updates.push(AcpSessionUpdate::AgentMessage {
                        message_id: ids.allocate(),
                        content: vec![AcpContentBlock::Text {
                            text: text_of(content),
                        }],
                    });
                }
                AgentMessage::User { content, .. } => {
                    updates.push(AcpSessionUpdate::UserMessage {
                        message_id: ids.allocate(),
                        content: user_blocks(content),
                    });
                }
                AgentMessage::Custom {
                    custom_type,
                    content,
                    details,
                    ..
                } => updates.push(extension_of(custom_type, content, details.as_ref())),
                _ => {}
            },
            Entry::Compaction { summary, .. } => {
                updates.push(extension(
                    "_yi/compaction",
                    [("summary", Value::String(summary.clone()))],
                ));
            }
            _ => {}
        }
    }
    updates
}
