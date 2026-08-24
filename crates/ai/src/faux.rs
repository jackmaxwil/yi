use serde_json::{Map, Value};
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, Content, StopReason, Usage};

pub const FAUX_API: &str = "faux";
pub const FAUX_PROVIDER: &str = "faux";
pub const FAUX_MODEL_ID: &str = "faux-1";
const CHUNK_CHARS: usize = 16;

pub fn zero_usage() -> Usage {
    Usage::zero()
}

pub fn faux_text(text: &str) -> Content {
    Content::Text {
        text: text.to_owned(),
        text_signature: None,
    }
}

pub fn faux_thinking(thinking: &str) -> Content {
    Content::Thinking {
        thinking: thinking.to_owned(),
        thinking_signature: None,
        redacted: None,
    }
}

pub fn faux_tool_call(id: &str, name: &str, arguments: Map<String, Value>) -> Content {
    Content::ToolCall {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments,
        thought_signature: None,
        namespace: None,
    }
}

pub fn faux_assistant_message(content: Vec<Content>, stop_reason: StopReason) -> AgentMessage {
    AgentMessage::Assistant {
        content,
        api: FAUX_API.to_owned(),
        provider: FAUX_PROVIDER.to_owned(),
        model: FAUX_MODEL_ID.to_owned(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: zero_usage(),
        stop_reason,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
    }
}

fn error_message(text: &str) -> AgentMessage {
    let mut message = faux_assistant_message(Vec::new(), StopReason::Error);
    if let AgentMessage::Assistant { error_message, .. } = &mut message {
        *error_message = Some(text.to_owned());
    }
    message
}

fn chunks(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    let characters: Vec<char> = text.chars().collect();
    characters
        .chunks(CHUNK_CHARS)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

fn with_partial(
    message: &AgentMessage,
    content: Vec<Content>,
    stop_reason: StopReason,
) -> AgentMessage {
    let mut partial = message.clone();
    if let AgentMessage::Assistant {
        content: partial_content,
        stop_reason: partial_stop,
        ..
    } = &mut partial
    {
        *partial_content = content;
        *partial_stop = stop_reason;
    }
    partial
}

pub fn stream_with_deltas(message: &AgentMessage) -> Vec<AssistantMessageEvent> {
    let AgentMessage::Assistant {
        content,
        stop_reason,
        ..
    } = message
    else {
        return vec![AssistantMessageEvent::Error {
            reason: StopReason::Error,
            error: error_message("faux response must be an assistant message"),
        }];
    };
    let mut events = Vec::new();
    let mut built: Vec<Content> = Vec::new();
    let partial = |built: &[Content]| with_partial(message, built.to_vec(), StopReason::Pending);
    events.push(AssistantMessageEvent::Start {
        partial: partial(&built),
    });

    for (index, block) in content.iter().enumerate() {
        match block {
            Content::Thinking { thinking, .. } => {
                built.push(faux_thinking(""));
                events.push(AssistantMessageEvent::ThinkingStart {
                    content_index: index,
                    partial: partial(&built),
                });
                let mut accumulated = String::new();
                for chunk in chunks(thinking) {
                    accumulated.push_str(&chunk);
                    built[index] = faux_thinking(&accumulated);
                    events.push(AssistantMessageEvent::ThinkingDelta {
                        content_index: index,
                        delta: chunk,
                        partial: partial(&built),
                    });
                }
                events.push(AssistantMessageEvent::ThinkingEnd {
                    content_index: index,
                    content: thinking.clone(),
                    partial: partial(&built),
                });
            }
            Content::Text { text, .. } => {
                built.push(faux_text(""));
                events.push(AssistantMessageEvent::TextStart {
                    content_index: index,
                    partial: partial(&built),
                });
                let mut accumulated = String::new();
                for chunk in chunks(text) {
                    accumulated.push_str(&chunk);
                    built[index] = faux_text(&accumulated);
                    events.push(AssistantMessageEvent::TextDelta {
                        content_index: index,
                        delta: chunk,
                        partial: partial(&built),
                    });
                }
                events.push(AssistantMessageEvent::TextEnd {
                    content_index: index,
                    content: text.clone(),
                    partial: partial(&built),
                });
            }
            Content::ToolCall {
                id,
                name,
                arguments,
                ..
            } => {
                built.push(faux_tool_call(id, name, Map::new()));
                events.push(AssistantMessageEvent::ToolCallStart {
                    content_index: index,
                    partial: partial(&built),
                });
                let serialized = Value::Object(arguments.clone()).to_string();
                for chunk in chunks(&serialized) {
                    events.push(AssistantMessageEvent::ToolCallDelta {
                        content_index: index,
                        delta: chunk,
                        partial: partial(&built),
                    });
                }
                built[index] = block.clone();
                events.push(AssistantMessageEvent::ToolCallEnd {
                    content_index: index,
                    tool_call: block.clone(),
                    partial: partial(&built),
                });
            }
            Content::Image { .. } => {}
        }
    }

    match stop_reason {
        StopReason::Error | StopReason::Aborted => events.push(AssistantMessageEvent::Error {
            reason: *stop_reason,
            error: message.clone(),
        }),
        reason => events.push(AssistantMessageEvent::Done {
            reason: *reason,
            message: message.clone(),
        }),
    }
    events
}

#[derive(Debug, Default)]
pub struct FauxProvider {
    queue: Vec<AgentMessage>,
    pub call_count: u64,
}

impl FauxProvider {
    pub fn set_responses(&mut self, responses: Vec<AgentMessage>) {
        self.queue = responses;
    }

    pub fn append_responses(&mut self, responses: Vec<AgentMessage>) {
        self.queue.extend(responses);
    }

    pub fn pending_response_count(&self) -> usize {
        self.queue.len()
    }

    pub fn stream(&mut self) -> Vec<AssistantMessageEvent> {
        self.call_count += 1;
        if self.queue.is_empty() {
            let error = error_message("No more faux responses queued");
            return vec![AssistantMessageEvent::Error {
                reason: StopReason::Error,
                error,
            }];
        }
        let message = self.queue.remove(0);
        stream_with_deltas(&message)
    }
}
