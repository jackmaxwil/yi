use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use agent_client_protocol_schema::v2 as acp;
use serde_json::Value;
use yi_types::encoding::base64;
use yi_types::entry::Entry;
use yi_types::event::{AgentEvent, AssistantMessageEvent};
use yi_types::goal::Goal;
use yi_types::message::{AgentMessage, Attribution, Content, StopReason, UserContent, shown_text};
use yi_types::subagent::ChildId;

pub fn extension<K: Into<String>>(
    kind: &str,
    fields: impl IntoIterator<Item = (K, Value)>,
) -> acp::SessionUpdate {
    acp::SessionUpdate::Other(acp::OtherSessionUpdate::new(
        kind,
        fields
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .collect(),
    ))
}

/// The runtime event verbatim: what a yi client reduces with solo's own reducer.
pub fn event_update(event: &AgentEvent, seq: u64, child: Option<&ChildId>) -> acp::SessionUpdate {
    let mut fields = vec![
        ("event", serde_json::to_value(event).unwrap_or(Value::Null)),
        ("seq", Value::from(seq)),
    ];
    if let Some(child) = child {
        fields.push(("childId", Value::String(child.0.clone())));
    }
    extension("_yi/event", fields)
}

pub fn gap_update(seq: u64, dropped: u64, child: Option<&ChildId>) -> acp::SessionUpdate {
    let mut fields = vec![("seq", Value::from(seq)), ("dropped", Value::from(dropped))];
    if let Some(child) = child {
        fields.push(("childId", Value::String(child.0.clone())));
    }
    extension("_yi/event_gap", fields)
}

/// Built by moving values into maps: `json!` of a struct re-serializes every nested
/// `Value`, which deep-copied a replay's entries twice per frame.
pub fn update_notification(session_id: &str, update: acp::SessionUpdate) -> Value {
    let update = match update {
        acp::SessionUpdate::Other(extension) => {
            let kind = ("sessionUpdate".to_owned(), extension.session_update.into());
            Value::Object(std::iter::once(kind).chain(extension.fields).collect())
        }
        other => serde_json::to_value(other).unwrap_or(Value::Null),
    };
    let params = object([("sessionId", session_id.into()), ("update", update)]);
    object([
        ("jsonrpc", "2.0".into()),
        ("method", "session/update".into()),
        ("params", params),
    ])
}

pub(crate) fn result_envelope(id: Value, result: Value) -> Value {
    object([("jsonrpc", "2.0".into()), ("id", id), ("result", result)])
}

fn object<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    Value::Object(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
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
    /// The ledger's totals, set by the client rather than added to; sent on a root's last frame.
    pub stats: Option<&'a yi_types::wire::SessionStats>,
}

pub fn replay_update(frame: &ReplayFrame<'_>) -> acp::SessionUpdate {
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
    if let Some(stats) = frame
        .stats
        .and_then(|stats| serde_json::to_value(stats).ok())
    {
        fields.push(("stats", stats));
    }
    extension("_yi/replay", fields)
}

#[derive(Debug)]
pub struct PendingPrompt {
    pub text: String,
    pub message_id: String,
    /// None once answered at queueing; the entry stays so the inserted message keeps its id.
    pub request: Option<Value>,
}

pub type PendingPrompts = Arc<Mutex<Vec<PendingPrompt>>>;

/// A poisoned list is still a list: a prompt dropped with it would never be answered.
pub fn queued(prompts: &PendingPrompts) -> std::sync::MutexGuard<'_, Vec<PendingPrompt>> {
    prompts
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Per-session translation state (design §17.2): message ids are allocated
/// here, tool-call and terminal ids pass through from the event stream.
#[derive(Debug, Default)]
pub struct IdMap {
    next_message: u64,
    current_message: Option<String>,
    last_stop: Option<acp::StopReason>,
    context_window: u64,
    prompts: PendingPrompts,
    inserted: Vec<PendingPrompt>,
}

impl IdMap {
    pub fn new(context_window: u64) -> Self {
        Self {
            context_window,
            ..Self::default()
        }
    }

    pub fn with_prompts(context_window: u64, prompts: PendingPrompts) -> Self {
        Self {
            prompts,
            ..Self::new(context_window)
        }
    }

    pub fn take_inserted(&mut self) -> Vec<PendingPrompt> {
        std::mem::take(&mut self.inserted)
    }

    pub fn waiting(&self) -> &PendingPrompts {
        &self.prompts
    }

    /// Two queued prompts of equal text may trade ids; both ids still land on a message.
    fn claim(&mut self, text: &str) -> Option<String> {
        let mut prompts = queued(&self.prompts);
        let index = prompts.iter().position(|prompt| prompt.text == text)?;
        let prompt = prompts.remove(index);
        let id = prompt.message_id.clone();
        self.inserted.push(prompt);
        self.current_message = Some(id.clone());
        Some(id)
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

fn stop_reason(reason: &StopReason) -> acp::StopReason {
    match reason {
        StopReason::Stop
        | StopReason::ToolUse
        | StopReason::Pending
        | StopReason::Deferred
        | StopReason::Other(_) => acp::StopReason::EndTurn,
        StopReason::Length => acp::StopReason::MaxTokens,
        StopReason::Aborted => acp::StopReason::Cancelled,
        StopReason::Error => {
            acp::StopReason::Other(acp::OtherStopReason::new("_yi/error", BTreeMap::new()))
        }
    }
}

fn tool_kind(tool_name: &str) -> acp::ToolKind {
    match tool_name {
        "read" | "glob" => acp::ToolKind::Read,
        "edit" | "write" => acp::ToolKind::Edit,
        "grep" | "web_search" => acp::ToolKind::Search,
        "bash" | "ipython" => acp::ToolKind::Execute,
        _ => acp::ToolKind::Other,
    }
}

fn text_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(shown_text)
        .collect::<Vec<_>>()
        .concat()
}

fn image_block(block: &Content) -> Option<acp::ContentBlock> {
    let Content::Image { data, mime_type } = block else {
        return None;
    };
    Some(acp::ContentBlock::Image(acp::ImageContent::new(
        data.clone(),
        mime_type.clone(),
    )))
}

fn host_notice_text(text: String) -> acp::ContentBlock {
    let mut meta = acp::Meta::new();
    meta.insert("yi".to_owned(), serde_json::json!({"hostNotice": true}));
    acp::ContentBlock::Text(acp::TextContent::new(text).meta(meta))
}

pub(crate) fn plain(content: &UserContent, sep: &str) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => yi_types::message::join_text(blocks, sep),
    }
}

fn user_blocks(content: &UserContent) -> Vec<acp::ContentBlock> {
    match content {
        UserContent::Text(text) => vec![text.clone().into()],
        UserContent::Blocks(blocks) => blocks
            .iter()
            .filter_map(shown_text)
            .map(|text| text.to_owned().into())
            .collect(),
    }
}

fn user_update(content: &UserContent, typed: bool, ids: &mut IdMap) -> acp::SessionUpdate {
    let claimed = typed.then(|| ids.claim(&plain(content, ""))).flatten();
    let message_id = claimed.unwrap_or_else(|| ids.allocate());
    let content = user_blocks(content);
    if typed {
        return acp::SessionUpdate::UserMessage(acp::UserMessage::new(message_id).content(content));
    }
    let content: Vec<_> = content
        .into_iter()
        .map(|block| match block {
            acp::ContentBlock::Text(text) => host_notice_text(text.text),
            other => other,
        })
        .collect();
    acp::SessionUpdate::AgentMessage(acp::AgentMessage::new(message_id).content(content))
}

/// A compaction note is host text the user must read: it rides as a host notice, which every
/// client shows, not as an extension a third-party client drops.
fn custom_update(
    custom_type: &str,
    content: &UserContent,
    details: Option<&Value>,
    ids: &mut IdMap,
) -> acp::SessionUpdate {
    if custom_type == yi_runtime::compaction::COMPACTION_NOTICE {
        return user_update(content, false, ids);
    }
    extension_of(custom_type, content, details)
}

/// Wraps a Custom message as a `_yi/<custom_type>` extension update (§17.2).
fn extension_of(
    custom_type: &str,
    content: &UserContent,
    details: Option<&Value>,
) -> acp::SessionUpdate {
    let mut fields = vec![("text", Value::String(plain(content, "")))];
    if let Some(details) = details {
        fields.push(("details", details.clone()));
    }
    extension(&format!("_yi/{custom_type}"), fields)
}

fn state(update: acp::StateUpdate) -> acp::SessionUpdate {
    acp::SessionUpdate::StateUpdate(update)
}

fn tool_call_update(
    tool_call_id: &str,
    status: acp::ToolCallStatus,
    raw_output: Option<Value>,
    content: Option<Vec<acp::ToolCallContent>>,
) -> acp::SessionUpdate {
    acp::SessionUpdate::ToolCallUpdate(
        acp::ToolCallUpdate::new(tool_call_id)
            .status(status)
            .content(content)
            .raw_output(raw_output),
    )
}

fn tool_status(result: &yi_types::event::ToolResult, is_error: bool) -> acp::ToolCallStatus {
    let kind = yi_types::event::ToolErrorKind::from_details(&result.details);
    let aborted = kind == Some(yi_types::event::ToolErrorKind::Aborted.as_str())
        || kind == Some(yi_types::event::ToolErrorKind::NotRun.as_str());
    let cut = result.details.get("cancelled").and_then(Value::as_bool) == Some(true);
    match (
        aborted || cut,
        yi_types::event::shown_failed(is_error, &result.details),
    ) {
        (true, _) => acp::ToolCallStatus::Cancelled,
        (false, true) => acp::ToolCallStatus::Failed,
        (false, false) => acp::ToolCallStatus::Completed,
    }
}

/// Exhaustive over `AgentEvent` so a new variant fails compile, not wire.
pub fn to_updates(event: &AgentEvent, ids: &mut IdMap) -> Vec<acp::SessionUpdate> {
    match event {
        AgentEvent::AgentStart => vec![state(acp::StateUpdate::Running(
            acp::RunningStateUpdate::new(),
        ))],
        AgentEvent::AgentEnd { .. } => vec![state(acp::StateUpdate::Idle(
            acp::IdleStateUpdate::new()
                .stop_reason(ids.last_stop.take().unwrap_or(acp::StopReason::EndTurn)),
        ))],
        AgentEvent::TurnStart | AgentEvent::TurnEnd { .. } => Vec::new(),
        AgentEvent::MessageStart { message } => match message {
            AgentMessage::Assistant { .. } => {
                ids.allocate();
                Vec::new()
            }
            AgentMessage::User {
                content,
                attribution,
                ..
            } => vec![user_update(content, *attribution == Attribution::User, ids)],
            AgentMessage::Custom {
                custom_type,
                content,
                details,
                ..
            } => vec![custom_update(custom_type, content, details.as_ref(), ids)],
            _ => Vec::new(),
        },
        AgentEvent::MessageUpdate {
            assistant_message_event,
            ..
        } => match assistant_message_event {
            AssistantMessageEvent::TextDelta { delta, .. } => {
                vec![acp::SessionUpdate::AgentMessageChunk(
                    acp::ContentChunk::new(delta.clone().into(), ids.current()),
                )]
            }
            AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                vec![acp::SessionUpdate::AgentThoughtChunk(
                    acp::ContentChunk::new(delta.clone().into(), ids.current()),
                )]
            }
            AssistantMessageEvent::Done { reason, .. }
            | AssistantMessageEvent::Error { reason, .. } => {
                ids.last_stop = Some(stop_reason(reason));
                Vec::new()
            }
            _ => Vec::new(),
        },
        AgentEvent::MessageEnd { message } => {
            ids.current_message = None;
            yi_runtime::reply_tokens(message)
                .map(|used| {
                    acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(
                        used.0,
                        ids.context_window,
                    ))
                })
                .into_iter()
                .collect()
        }
        AgentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => vec![acp::SessionUpdate::ToolCallUpdate(
            acp::ToolCallUpdate::new(tool_call_id.as_str())
                .name(tool_name.clone())
                .title(tool_name.clone())
                .kind(tool_kind(tool_name))
                .status(acp::ToolCallStatus::InProgress)
                .raw_input(args.clone()),
        )],
        AgentEvent::ToolExecutionEnd {
            tool_call_id,
            tool_name,
            result,
            is_error,
        } => {
            let status = tool_status(result, *is_error);
            let text = text_of(&result.content);
            if tool_name == "bash" {
                let exit_code = result
                    .details
                    .get("exitCode")
                    .and_then(Value::as_u64)
                    .and_then(|code| u32::try_from(code).ok());
                vec![
                    acp::SessionUpdate::TerminalOutputChunk(acp::TerminalOutputChunk::new(
                        tool_call_id.as_str(),
                        base64(text.as_bytes()),
                    )),
                    acp::SessionUpdate::TerminalUpdate(
                        acp::TerminalUpdate::new(tool_call_id.as_str()).exit_status(
                            acp::TerminalExitStatus::new().exit_code(exit_code.map(i64::from)),
                        ),
                    ),
                    tool_call_update(
                        tool_call_id,
                        status,
                        None,
                        Some(vec![acp::Terminal::new(tool_call_id.as_str()).into()]),
                    ),
                ]
            } else {
                vec![tool_call_update(
                    tool_call_id,
                    status,
                    Some(result.details.clone()),
                    Some(
                        std::iter::once(acp::ContentBlock::from(text))
                            .chain(result.content.iter().filter_map(image_block))
                            .map(acp::ToolCallContent::from)
                            .collect(),
                    ),
                )]
            }
        }
        AgentEvent::PermissionRequested { .. } => {
            vec![state(acp::StateUpdate::RequiresAction(
                acp::RequiresActionStateUpdate::new(),
            ))]
        }
        AgentEvent::PermissionResolved { .. } => {
            vec![state(acp::StateUpdate::Running(
                acp::RunningStateUpdate::new(),
            ))]
        }
        AgentEvent::ChildUpdate { update } => {
            object_extension("_yi/subagent_update", serde_json::to_value(update))
        }
        AgentEvent::LandingState { landing } => {
            object_extension("_yi/landing", serde_json::to_value(landing))
        }
        AgentEvent::ToolExecutionUpdate {
            tool_call_id,
            partial_result,
            ..
        } => vec![tool_progress(tool_call_id, partial_result)],
        AgentEvent::Wait { .. } | AgentEvent::Other(_) => Vec::new(),
    }
}

fn tool_progress(tool_call_id: &str, partial: &yi_types::event::ToolResult) -> acp::SessionUpdate {
    let stream = partial.details.get("stream").cloned();
    extension(
        "_yi/tool_progress",
        [
            ("toolCallId", Value::from(tool_call_id)),
            ("text", Value::from(text_of(&partial.content))),
            ("stream", stream.unwrap_or(Value::Null)),
        ],
    )
}

fn object_extension(name: &str, value: serde_json::Result<Value>) -> Vec<acp::SessionUpdate> {
    let fields = match value {
        Ok(Value::Object(fields)) => fields,
        _ => serde_json::Map::new(),
    };
    vec![extension(name, fields)]
}

/// Replay (design §17.2): a stored branch walked into full-message updates.
pub fn replay_updates(entries: &[Entry], ids: &mut IdMap) -> Vec<acp::SessionUpdate> {
    let mut updates = Vec::new();
    for entry in entries {
        match entry {
            Entry::Message {
                message, timestamp, ..
            } => match message {
                AgentMessage::Assistant { content, .. } => {
                    updates.push(acp::SessionUpdate::AgentMessage(
                        acp::AgentMessage::new(ids.allocate())
                            .content(vec![acp::ContentBlock::from(text_of(content))]),
                    ));
                }
                AgentMessage::User {
                    content,
                    attribution,
                    ..
                } => updates.push(user_update(
                    content,
                    attribution.reads_as_typed(*timestamp),
                    ids,
                )),
                AgentMessage::Custom {
                    custom_type,
                    content,
                    details,
                    ..
                } => updates.push(custom_update(custom_type, content, details.as_ref(), ids)),
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
