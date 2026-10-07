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

pub use proto::{CLIENT_VERSION, NAMESPACE};

pub type ToolFuture = Pin<Box<dyn Future<Output = String> + Send>>;
pub type ToolFn = Box<dyn Fn(String, String, Map<String, Value>) -> ToolFuture + Send + Sync>;
pub type StopFn = Arc<dyn Fn() -> bool + Send + Sync>;

pub struct Applied {
    pub events: Vec<AssistantMessageEvent>,
    pub replies: Vec<Vec<u8>>,
}

pub fn apply_message(
    message: &[u8],
    tool: &mut dyn FnMut(&str, &Map<String, Value>) -> String,
) -> Applied {
    let mut events = Vec::new();
    let mut replies = Vec::new();
    match proto::decode(message) {
        Ok(proto::Inbound::Text(text)) => events.push(AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: text,
        }),
        Ok(proto::Inbound::Thinking(text)) => events.push(AssistantMessageEvent::ThinkingDelta {
            content_index: 0,
            delta: text,
        }),
        Ok(proto::Inbound::TurnEnded)
        | Ok(proto::Inbound::Ignore)
        | Ok(proto::Inbound::Context { .. })
        | Ok(proto::Inbound::KvGet { .. })
        | Ok(proto::Inbound::KvSet { .. }) => {}
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
            let output = tool(&name, &args);
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

pub(crate) fn tool_event(
    id: &str,
    name: &str,
    arguments: Map<String, Value>,
) -> AssistantMessageEvent {
    AssistantMessageEvent::ToolCallEnd {
        content_index: 0,
        tool_call: Content::ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments,
            thought_signature: None,
            namespace: Some(NAMESPACE.to_owned()),
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
            AgentMessage::User { content, .. } => parts.push(user_text(content)),
            AgentMessage::Assistant { content, .. } => parts.push(assistant_text(content)),
            _ => {}
        }
    }
    parts.join("\n")
}

fn user_text(content: &yi_types::message::UserContent) -> String {
    match content {
        yi_types::message::UserContent::Text(text) => text.clone(),
        yi_types::message::UserContent::Blocks(blocks) => assistant_text(blocks),
    }
}

fn assistant_text(blocks: &[Content]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.clone()),
            Content::ToolCall { name, .. } => Some(format!("[tool {name}]")),
            _ => None,
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

pub fn finish(model: &Model, events: &[AssistantMessageEvent]) -> AgentMessage {
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
    let mut text = String::new();
    let mut thinking = String::new();
    let mut calls = Vec::new();
    for event in events {
        match event {
            AssistantMessageEvent::TextDelta { delta, .. } => text.push_str(delta),
            AssistantMessageEvent::ThinkingDelta { delta, .. } => thinking.push_str(delta),
            AssistantMessageEvent::ToolCallEnd { tool_call, .. } => calls.push(tool_call.clone()),
            _ => {}
        }
    }
    if !thinking.is_empty() {
        content.push(Content::Thinking {
            thinking,
            thinking_signature: None,
            redacted: None,
        });
    }
    if !text.is_empty() {
        content.push(Content::Text {
            text,
            text_signature: None,
        });
    }
    content.extend(calls);
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
) -> tokio::sync::mpsc::Receiver<AssistantMessageEvent> {
    let (sender, receiver) = tokio::sync::mpsc::channel(32);
    let model = model.clone();
    let tools = tools_of(context);
    let workspace = std::env::current_dir()
        .map(|path| format!("file://{}", path.display()))
        .unwrap_or_else(|_| "file:///".to_owned());
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
                let done = finish(&model, blocks.raw());
                let _ = sender
                    .send(AssistantMessageEvent::Done {
                        reason: stop_of(&done),
                        message: done,
                    })
                    .await;
            }
            Err(error) => {
                let mut output = empty_assistant(&model);
                let _ = sender.send(fail_message(&mut output, &error)).await;
            }
        }
    }));
    receiver
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

#[derive(Default)]
pub(super) struct Blocks {
    text_open: bool,
    thinking_open: bool,
    text: String,
    thinking: String,
    raw: Vec<AssistantMessageEvent>,
}

impl Blocks {
    fn stage(&mut self, event: &AssistantMessageEvent) -> Vec<AssistantMessageEvent> {
        let mut out = Vec::new();
        match event {
            AssistantMessageEvent::TextDelta { delta, .. } => {
                if !self.text_open {
                    self.text_open = true;
                    out.push(AssistantMessageEvent::TextStart { content_index: 1 });
                }
                self.text.push_str(delta);
                out.push(AssistantMessageEvent::TextDelta {
                    content_index: 1,
                    delta: delta.clone(),
                });
            }
            AssistantMessageEvent::ThinkingDelta { delta, .. } => {
                if !self.thinking_open {
                    self.thinking_open = true;
                    out.push(AssistantMessageEvent::ThinkingStart { content_index: 0 });
                }
                self.thinking.push_str(delta);
                out.push(event.clone());
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
        self.raw.push(event);
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
        let mut out = Vec::new();
        if self.thinking_open {
            self.thinking_open = false;
            out.push(AssistantMessageEvent::ThinkingEnd {
                content_index: 0,
                content: std::mem::take(&mut self.thinking),
            });
        }
        if self.text_open {
            self.text_open = false;
            out.push(AssistantMessageEvent::TextEnd {
                content_index: 1,
                content: std::mem::take(&mut self.text),
            });
        }
        out
    }

    fn raw(&self) -> &[AssistantMessageEvent] {
        &self.raw
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
    use super::{AssistantMessageEvent, text_events};

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
            Some(AssistantMessageEvent::TextStart { content_index: 1 })
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
