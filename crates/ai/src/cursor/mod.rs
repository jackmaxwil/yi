pub mod proto;
mod wire;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, Content, StopReason, Usage};
use yi_types::model::{LlmContext, Model};

use crate::request::{empty_assistant, fail_message};

pub use proto::CLIENT_VERSION;

pub const BASE_URL: &str = "https://agentn.us.api5.cursor.sh";
pub const MODELS_URL: &str = "https://api2.cursor.sh/aiserver.v1.AiService/AvailableModels";

pub fn model_for(id: &str) -> Model {
    use yi_types::model::ModelCost;
    Model {
        id: id.to_owned(),
        name: id.to_owned(),
        api: "cursor-agent".to_owned(),
        provider: "cursor".to_owned(),
        base_url: BASE_URL.to_owned(),
        reasoning: true,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: serde_json::Number::from(0),
            output: serde_json::Number::from(0),
            cache_read: serde_json::Number::from(0),
            cache_write: serde_json::Number::from(0),
            tiers: None,
        },
        context_window: 200_000,
        max_tokens: 16_000,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

pub type ToolFuture = Pin<Box<dyn Future<Output = String> + Send>>;
pub type ToolFn = Box<dyn Fn(String, String, Map<String, Value>) -> ToolFuture + Send + Sync>;
pub type StopFn = Arc<dyn Fn() -> bool + Send + Sync>;

pub struct Applied {
    pub events: Vec<AssistantMessageEvent>,
    pub replies: Vec<Vec<u8>>,
}

pub type Blobs = std::collections::HashMap<Vec<u8>, Vec<u8>>;

pub async fn apply_message(
    inbound: Result<proto::Inbound, String>,
    tool: &ToolFn,
    tools: &[proto::ToolWire],
    workspace: &str,
    blobs: &mut Blobs,
) -> Applied {
    let mut events = Vec::new();
    let mut replies = Vec::new();
    match inbound {
        Ok(proto::Inbound::Text(text)) => events.push(AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: text,
        }),
        Ok(proto::Inbound::Thinking(text)) => events.push(AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: text,
        }),
        Ok(proto::Inbound::TurnEnded) | Ok(proto::Inbound::Ignore) => {}
        Ok(proto::Inbound::KvSet { id, blob_id, data }) => {
            blobs.insert(blob_id, data);
            replies.push(proto::kv_set_reply(id));
        }
        Ok(proto::Inbound::KvGet { id, blob_id }) => {
            let data = blobs.get(&blob_id).cloned().unwrap_or_default();
            replies.push(proto::kv_get_reply(id, &data));
        }
        Ok(proto::Inbound::Context { id, exec_id }) => {
            replies.push(proto::context_reply(id, &exec_id, workspace, tools));
        }
        Ok(proto::Inbound::Tool {
            id,
            name,
            call_id,
            exec_id,
            args,
            route,
            command,
            cwd,
        }) => {
            let output = tool(call_id.clone(), name.clone(), args.clone()).await;
            events.push(tool_event(&call_id, &name, args));
            replies.extend(proto::tool_frames(
                route, id, &exec_id, &command, &cwd, &output,
            ));
        }
        Ok(proto::Inbound::Reject { id }) => {
            replies.push(proto::throw_reply(id, "yi does not run this cursor tool"));
            replies.push(proto::stream_close(id));
        }
        Err(error) => {
            replies.push(proto::throw_reply(0, &error));
            replies.push(proto::stream_close(0));
        }
    }
    Applied { events, replies }
}

fn tool_event(id: &str, name: &str, arguments: Map<String, Value>) -> AssistantMessageEvent {
    AssistantMessageEvent::ToolCallEnd {
        content_index: 0,
        tool_call: Content::ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments,
            thought_signature: None,
            namespace: None,
        },
    }
}

pub fn prompt_of(context: &LlmContext) -> String {
    let mut parts = Vec::new();
    if !context.system_prompt.is_empty() {
        parts.push(context.system_prompt.clone());
    }
    for message in context.messages.iter().chain(context.transient.iter()) {
        match message {
            AgentMessage::User {
                content: yi_types::message::UserContent::Text(text),
                ..
            } => parts.push(format!("User: {text}")),
            AgentMessage::User {
                content: yi_types::message::UserContent::Blocks(blocks),
                ..
            } => parts.push(format!("User: {}", blocks_text(blocks))),
            AgentMessage::Assistant { content, .. } => {
                parts.push(format!("Assistant: {}", blocks_text(content)));
            }
            AgentMessage::ToolResult { tool_name, .. } => parts.push(format!(
                "[… result of {tool_name} dropped: the cursor provider does not receive tool results]"
            )),
            _ => {}
        }
    }
    parts.join("\n")
}

fn blocks_text(blocks: &[Content]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.clone()),
            Content::ToolCall { name, .. } => Some(format!("[tool {name}]")),
            Content::Image { .. } => {
                Some("[… image dropped: the cursor provider receives text only]".to_owned())
            }
            Content::Thinking { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tools_of(context: &LlmContext) -> Vec<proto::ToolWire> {
    context
        .tools
        .iter()
        .flatten()
        .map(|tool| proto::ToolWire {
            name: tool.name.clone(),
            description: tool.description.clone(),
            schema: tool.parameters.to_string(),
        })
        .collect()
}

fn finish(model: &Model, blocks: Blocks) -> AgentMessage {
    let mut message = empty_assistant(model);
    let AgentMessage::Assistant {
        content,
        stop_reason,
        usage,
        ..
    } = &mut message
    else {
        return message;
    };
    *usage = Usage::zero();
    content.extend(blocks.content);
    *stop_reason = if content
        .iter()
        .any(|block| matches!(block, Content::ToolCall { .. }))
    {
        StopReason::ToolUse
    } else {
        StopReason::Stop
    };
    message
}

pub fn stream(
    model: &Model,
    context: &LlmContext,
    token: &str,
    tool: ToolFn,
    stop: StopFn,
    cwd: Option<&std::path::Path>,
) -> tokio::sync::mpsc::Receiver<AssistantMessageEvent> {
    let (sender, receiver) = tokio::sync::mpsc::channel(32);
    let model = model.clone();
    let tools = tools_of(context);
    let workspace = (cwd.map(std::path::Path::to_path_buf))
        .or_else(|| std::env::current_dir().ok())
        .map_or_else(
            || "file:///".to_owned(),
            |path| format!("file://{}", path.display()),
        );
    let body = proto::frame(&proto::run_request(
        &prompt_of(context),
        &model.id,
        &uuid(),
        &tools,
        &workspace,
    ));
    let base = model.base_url.clone();
    let token = token.to_owned();
    drop(tokio::spawn(async move {
        if sender
            .send(AssistantMessageEvent::Start {
                partial: empty_assistant(&model),
            })
            .await
            .is_err()
        {
            return;
        }
        let mut blocks = Blocks::default();
        let ran = {
            let mut out = Out {
                sink: &sender,
                blocks: &mut blocks,
                stop: &stop,
            };
            wire::run(&base, &token, body, &tool, &tools, &workspace, &mut out).await
        };
        match ran {
            Ok(()) => {
                if !blocks.close(&sender).await {
                    return;
                }
                let done = finish(&model, blocks);
                let _ = sender
                    .send(AssistantMessageEvent::Done {
                        reason: stop_of(&done),
                        message: done,
                    })
                    .await;
            }
            Err(error) => {
                let _ = sender.send(failed(&model, blocks, &error)).await;
            }
        }
    }));
    receiver
}

/// Incident: an empty failed message read as nothing delivered, and the loop's stream retry
/// re-ran tools this turn had already run.
fn failed(model: &Model, blocks: Blocks, error: &str) -> AssistantMessageEvent {
    let mut output = finish(model, blocks);
    fail_message(&mut output, error)
}

fn stop_of(message: &AgentMessage) -> StopReason {
    match message {
        AgentMessage::Assistant { stop_reason, .. } => *stop_reason,
        _ => StopReason::Stop,
    }
}

pub(super) struct Out<'a> {
    sink: &'a tokio::sync::mpsc::Sender<AssistantMessageEvent>,
    blocks: &'a mut Blocks,
    stop: &'a StopFn,
}

/// Invariant: each block sits at the index its start event carried, so a folded partial is [`finish`].
#[derive(Default)]
pub(super) struct Blocks {
    text: Option<usize>,
    thinking: Option<usize>,
    content: Vec<Content>,
}

impl Blocks {
    fn open(&mut self, block: Content) -> usize {
        self.content.push(block);
        self.content.len().saturating_sub(1)
    }

    fn stage(&mut self, event: &AssistantMessageEvent) -> Vec<AssistantMessageEvent> {
        let mut out = Vec::new();
        match event {
            AssistantMessageEvent::TextDelta { delta, .. } => {
                let index = self.text.unwrap_or_else(|| {
                    let index = self.open(Content::Text {
                        text: String::new(),
                        text_signature: None,
                    });
                    out.push(AssistantMessageEvent::TextStart {
                        content_index: index,
                    });
                    index
                });
                self.text = Some(index);
                if let Some(Content::Text { text, .. }) = self.content.get_mut(index) {
                    text.push_str(delta);
                }
                out.push(AssistantMessageEvent::TextDelta {
                    content_index: index,
                    delta: delta.clone(),
                });
            }
            AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                let index = self.thinking.unwrap_or_else(|| {
                    let index = self.open(Content::Thinking {
                        thinking: String::new(),
                        thinking_signature: None,
                        redacted: None,
                    });
                    out.push(AssistantMessageEvent::ThinkingStart {
                        content_index: index,
                    });
                    index
                });
                self.thinking = Some(index);
                if let Some(Content::Thinking { thinking, .. }) = self.content.get_mut(index) {
                    thinking.push_str(delta);
                }
                out.push(AssistantMessageEvent::ThinkingDelta {
                    content_index: index,
                    delta: delta.clone(),
                });
            }
            AssistantMessageEvent::ToolCallEnd { tool_call, .. } => {
                // A call settles the blocks before it, so prose after it opens a block of its own.
                out.extend(self.closing());
                out.push(AssistantMessageEvent::ToolCallEnd {
                    content_index: self.open(tool_call.clone()),
                    tool_call: tool_call.clone(),
                });
            }
            other => out.push(other.clone()),
        }
        out
    }

    async fn push(
        &mut self,
        sender: &tokio::sync::mpsc::Sender<AssistantMessageEvent>,
        event: AssistantMessageEvent,
    ) -> bool {
        let staged = self.stage(&event);
        for event in staged {
            if sender.send(event).await.is_err() {
                return false;
            }
        }
        true
    }

    async fn close(&mut self, sender: &tokio::sync::mpsc::Sender<AssistantMessageEvent>) -> bool {
        for event in self.closing() {
            if sender.send(event).await.is_err() {
                return false;
            }
        }
        true
    }

    fn closing(&mut self) -> Vec<AssistantMessageEvent> {
        [self.thinking.take(), self.text.take()]
            .into_iter()
            .flatten()
            .filter_map(|index| match self.content.get(index) {
                Some(Content::Thinking { thinking, .. }) => {
                    Some(AssistantMessageEvent::ThinkingEnd {
                        content_index: index,
                        content: thinking.clone(),
                    })
                }
                Some(Content::Text { text, .. }) => Some(AssistantMessageEvent::TextEnd {
                    content_index: index,
                    content: text.clone(),
                }),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
fn text_events(events: &[AssistantMessageEvent]) -> Vec<AssistantMessageEvent> {
    let mut blocks = Blocks::default();
    let mut out = Vec::new();
    for event in events {
        out.extend(blocks.stage(event));
    }
    out.extend(blocks.closing());
    out
}

fn uuid() -> String {
    let Ok(bytes) = yi_oauth::pkce::random_bytes(16) else {
        return "cursor-turn".to_owned();
    };
    yi_oauth::pkce::hex(&bytes)
}

#[cfg(test)]
mod tests {
    use super::{
        AgentMessage, AssistantMessageEvent, Blocks, Map, empty_assistant, failed, finish,
        model_for, text_events, tool_event,
    };

    #[test]
    fn prose_after_a_tool_call_is_a_block_of_its_own() {
        let text = |delta: &str| AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: delta.to_owned(),
        };
        let mut blocks = Blocks::default();
        for event in [
            text("Listing."),
            tool_event("a", "bash", Map::new()),
            text("Two files."),
        ] {
            drop(blocks.stage(&event));
        }
        drop(blocks.closing());
        let AgentMessage::Assistant { content, .. } = finish(&model_for("m"), blocks) else {
            return;
        };
        let shape: Vec<String> = (content.iter())
            .map(|block| match block {
                yi_types::message::Content::Text { text, .. } => text.clone(),
                yi_types::message::Content::ToolCall { name, .. } => format!("<{name}>"),
                _ => String::new(),
            })
            .collect();
        assert_eq!(shape, ["Listing.", "<bash>", "Two files."]);
    }

    #[test]
    fn a_stream_that_fails_after_a_tool_ran_still_carries_the_call()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut blocks = Blocks::default();
        drop(blocks.stage(&tool_event("a", "bash", Map::new())));
        let event = failed(
            &model_for("m"),
            blocks,
            "cursor stream closed before the turn ended",
        );
        let AssistantMessageEvent::Error {
            error: AgentMessage::Assistant { content, .. },
            ..
        } = event
        else {
            return Err("a failed turn is an Error event".into());
        };
        assert!(matches!(
            content.as_slice(),
            [yi_types::message::Content::ToolCall { id, .. }] if id == "a"
        ));
        Ok(())
    }

    #[test]
    fn the_partial_folded_from_staged_events_is_the_done_message()
    -> Result<(), Box<dyn std::error::Error>> {
        let model = model_for("m");
        let thinking = |delta: &str| AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: delta.to_owned(),
        };
        let text = |delta: &str| AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: delta.to_owned(),
        };
        let mut blocks = Blocks::default();
        let mut partial = empty_assistant(&model);
        let mut staged = Vec::new();
        for event in [
            thinking("plan"),
            text("Hi"),
            tool_event("a", "read", Map::new()),
            thinking(" more"),
            tool_event("b", "grep", Map::new()),
            text(" there"),
        ] {
            staged.extend(blocks.stage(&event));
        }
        staged.extend(blocks.closing());
        for event in &staged {
            yi_types::event::apply(&mut partial, event);
        }
        let done = finish(&model, blocks);
        let (
            AgentMessage::Assistant { content: built, .. },
            AgentMessage::Assistant { content: want, .. },
        ) = (&partial, &done)
        else {
            return Err("assistant messages".into());
        };
        assert_eq!(want.len(), 6);
        assert_eq!(built, want);
        Ok(())
    }

    #[test]
    fn a_delta_leaves_before_the_turn_closes() {
        let staged = text_events(&[
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: "Hi".to_owned(),
            },
            AssistantMessageEvent::TextDelta {
                content_index: 0,
                delta: " there".to_owned(),
            },
        ]);
        assert!(matches!(
            staged.first(),
            Some(AssistantMessageEvent::TextStart { content_index: 0 })
        ));
        assert!(matches!(
            staged.get(1),
            Some(AssistantMessageEvent::TextDelta { delta, .. }) if delta == "Hi"
        ));
        assert!(matches!(
            staged.last(),
            Some(AssistantMessageEvent::TextEnd { content, .. }) if content == "Hi there"
        ));
    }
}
