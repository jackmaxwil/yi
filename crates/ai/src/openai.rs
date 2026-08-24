use serde_json::{Value, json};
use tokio::sync::mpsc::{Receiver, Sender};
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, Content, StopReason, Usage, UserContent};
use yi_types::model::{LlmContext, Model, ToolDef};

use crate::catalog::calculate_cost;
use crate::json_salvage::{parse_json_with_repair, parse_streaming_json};
use crate::transform::transform_messages;

const REASONING_FIELDS: [&str; 3] = ["reasoning_content", "reasoning", "reasoning_text"];

#[derive(Debug, Clone, Default)]
pub struct OpenAiOptions {
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub reasoning_effort: Option<String>,
    pub session_id: Option<String>,
}

fn fnv1a(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

pub fn normalize_openai_tool_call_id(id: &str) -> String {
    let sanitize = |part: &str| -> String {
        part.chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                    character
                } else {
                    '_'
                }
            })
            .collect()
    };
    if let Some(separator) = id.find('|') {
        let call_id = sanitize(&id[..separator]);
        let item_id = sanitize(&id[separator.saturating_add(1)..]);
        let combined = if item_id.is_empty() {
            call_id.clone()
        } else {
            format!("{call_id}_{item_id}")
        };
        if combined.len() <= 40 {
            return combined;
        }
        let hash = format!("{:08x}", fnv1a(id) as u32);
        let prefix_len = 40usize.saturating_sub(hash.len().saturating_add(1)).max(1);
        let prefix: String = call_id.chars().take(prefix_len).collect();
        return format!("{prefix}_{hash}");
    }
    if id.len() > 40 {
        return id.chars().take(40).collect();
    }
    id.to_owned()
}

fn convert_messages(model: &Model, context: &LlmContext) -> Vec<Value> {
    let transformed = transform_messages(
        &context.messages,
        model,
        Some(normalize_openai_tool_call_id),
    );
    let mut params: Vec<Value> = Vec::new();
    if !context.system_prompt.is_empty() {
        let role = if model.reasoning {
            "developer"
        } else {
            "system"
        };
        params.push(json!({"role": role, "content": context.system_prompt}));
    }
    let mut index = 0;
    while index < transformed.len() {
        match &transformed[index] {
            AgentMessage::User { content, .. } => match content {
                UserContent::Text(text) => {
                    params.push(json!({"role": "user", "content": text}));
                }
                UserContent::Blocks(blocks) => {
                    let parts: Vec<Value> = blocks
                        .iter()
                        .filter_map(|block| match block {
                            Content::Text { text, .. } => {
                                Some(json!({"type": "text", "text": text}))
                            }
                            Content::Image { data, mime_type } => Some(json!({
                                "type": "image_url",
                                "image_url": {"url": format!("data:{mime_type};base64,{data}")},
                            })),
                            _ => None,
                        })
                        .collect();
                    if !parts.is_empty() {
                        params.push(json!({"role": "user", "content": parts}));
                    }
                }
            },
            AgentMessage::Assistant { content, .. } => {
                let text: String = content
                    .iter()
                    .filter_map(|block| match block {
                        Content::Text { text, .. } if !text.trim().is_empty() => {
                            Some(text.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let thinking: Vec<&Content> = content
                    .iter()
                    .filter(|block| {
                        matches!(block, Content::Thinking { thinking, .. } if !thinking.trim().is_empty())
                    })
                    .collect();
                let tool_calls: Vec<Value> = content
                    .iter()
                    .filter_map(|block| match block {
                        Content::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => Some(json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": Value::Object(arguments.clone()).to_string(),
                            },
                        })),
                        _ => None,
                    })
                    .collect();
                let mut message = json!({"role": "assistant", "content": Value::Null});
                if !text.is_empty() {
                    message["content"] = Value::String(text.clone());
                }
                if let Some(Content::Thinking {
                    thinking,
                    thinking_signature,
                    ..
                }) = thinking.first()
                    && let Some(field) = thinking_signature
                        .as_deref()
                        .filter(|signature| REASONING_FIELDS.contains(signature))
                {
                    message[field] = Value::String(thinking.clone());
                }
                if !tool_calls.is_empty() {
                    message["tool_calls"] = Value::Array(tool_calls);
                }
                let has_content = message["content"].is_string();
                if has_content || message.get("tool_calls").is_some() {
                    params.push(message);
                }
            }
            AgentMessage::ToolResult { .. } => {
                let mut images: Vec<Value> = Vec::new();
                while let Some(AgentMessage::ToolResult {
                    tool_call_id,
                    content,
                    ..
                }) = transformed.get(index)
                {
                    let text: String = content
                        .iter()
                        .filter_map(|block| match block {
                            Content::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let has_images = content
                        .iter()
                        .any(|block| matches!(block, Content::Image { .. }));
                    let body = if !text.is_empty() {
                        text
                    } else if has_images {
                        "(see attached image)".to_owned()
                    } else {
                        "(no tool output)".to_owned()
                    };
                    params.push(json!({
                        "role": "tool",
                        "content": body,
                        "tool_call_id": tool_call_id,
                    }));
                    if has_images && model.input.iter().any(|kind| kind == "image") {
                        for block in content {
                            if let Content::Image { data, mime_type } = block {
                                images.push(json!({
                                    "type": "image_url",
                                    "image_url": {"url": format!("data:{mime_type};base64,{data}")},
                                }));
                            }
                        }
                    }
                    index = index.saturating_add(1);
                }
                index = index.saturating_sub(1);
                if !images.is_empty() {
                    let mut parts = vec![
                        json!({"type": "text", "text": "Attached image(s) from tool result:"}),
                    ];
                    parts.extend(images);
                    params.push(json!({"role": "user", "content": parts}));
                }
            }
            _ => {}
        }
        index = index.saturating_add(1);
    }
    params
}

fn convert_tools(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                    "strict": false,
                },
            })
        })
        .collect()
}

pub fn build_params(model: &Model, context: &LlmContext, options: &OpenAiOptions) -> Value {
    let mut params = json!({
        "model": model.id,
        "messages": convert_messages(model, context),
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if model.provider == "openai" {
        params["store"] = json!(false);
    }
    if model.base_url.contains("api.openai.com")
        && let Some(session_id) = &options.session_id
    {
        params["prompt_cache_key"] = json!(session_id);
    }
    if let Some(max_tokens) = options.max_tokens {
        params["max_completion_tokens"] = json!(max_tokens);
    }
    if let Some(temperature) = options.temperature {
        params["temperature"] = json!(temperature);
    }
    if let Some(tools) = &context.tools
        && !tools.is_empty()
    {
        params["tools"] = Value::Array(convert_tools(tools));
    }
    if model.reasoning
        && let Some(effort) = &options.reasoning_effort
    {
        let mapped = model
            .thinking_level_map
            .as_ref()
            .and_then(|map| map.get(effort.as_str()))
            .and_then(Value::as_str)
            .unwrap_or(effort);
        params["reasoning_effort"] = json!(mapped);
    }
    params
}

fn map_finish_reason(reason: &str) -> (StopReason, Option<String>) {
    match reason {
        "stop" | "end" => (StopReason::Stop, None),
        "length" => (StopReason::Length, None),
        "function_call" | "tool_calls" => (StopReason::ToolUse, None),
        other => (
            StopReason::Error,
            Some(format!("Provider finish_reason: {other}")),
        ),
    }
}

fn parse_chunk_usage(raw: &Value, model: &Model) -> Usage {
    let get = |value: &Value, key: &str| value.get(key).and_then(Value::as_i64).unwrap_or(0);
    let prompt = get(raw, "prompt_tokens");
    let details = raw.get("prompt_tokens_details");
    let cache_read = details
        .and_then(|value| value.get("cached_tokens"))
        .and_then(Value::as_i64)
        .or_else(|| raw.get("prompt_cache_hit_tokens").and_then(Value::as_i64))
        .or_else(|| raw.get("cached_tokens").and_then(Value::as_i64))
        .unwrap_or(0);
    let cache_write = details
        .and_then(|value| value.get("cache_write_tokens"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let input = prompt
        .saturating_sub(cache_read)
        .saturating_sub(cache_write);
    let output = get(raw, "completion_tokens");
    let mut usage = Usage::zero();
    usage.input = input;
    usage.output = output;
    usage.cache_read = cache_read;
    usage.cache_write = cache_write;
    usage.reasoning = raw
        .get("completion_tokens_details")
        .and_then(|value| value.get("reasoning_tokens"))
        .and_then(Value::as_i64);
    usage.total_tokens = input
        .saturating_add(output)
        .saturating_add(cache_read)
        .saturating_add(cache_write);
    calculate_cost(model, &mut usage);
    usage
}

struct ToolSlot {
    content_index: usize,
    stream_index: Option<u64>,
    id: String,
    partial_args: String,
}

pub struct ChunkMapper {
    output: AgentMessage,
    model: Model,
    text_index: Option<usize>,
    thinking_index: Option<usize>,
    tools: Vec<ToolSlot>,
    has_finish_reason: bool,
}

impl ChunkMapper {
    pub fn new(model: &Model) -> Self {
        Self {
            output: crate::request::empty_assistant(model),
            model: model.clone(),
            text_index: None,
            thinking_index: None,
            tools: Vec::new(),
            has_finish_reason: false,
        }
    }

    fn content_mut(&mut self) -> &mut Vec<Content> {
        match &mut self.output {
            AgentMessage::Assistant { content, .. } => content,
            _ => unreachable!("mapper output is always an assistant message"),
        }
    }

    pub fn start_event(&self) -> AssistantMessageEvent {
        AssistantMessageEvent::Start {
            partial: self.output.clone(),
        }
    }

    pub fn push_chunk(&mut self, chunk: &Value) -> Vec<AssistantMessageEvent> {
        let mut events = Vec::new();
        let partial = |mapper: &Self| mapper.output.clone();
        if let Some(id) = chunk.get("id").and_then(Value::as_str)
            && let AgentMessage::Assistant { response_id, .. } = &mut self.output
            && response_id.is_none()
        {
            *response_id = Some(id.to_owned());
        }
        if let Some(usage_value) = chunk.get("usage").filter(|value| !value.is_null()) {
            let usage = parse_chunk_usage(usage_value, &self.model);
            if let AgentMessage::Assistant {
                usage: output_usage,
                ..
            } = &mut self.output
            {
                *output_usage = usage;
            }
        }
        let Some(choice) = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
        else {
            return events;
        };
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            let (stop_reason, error_message) = map_finish_reason(reason);
            if let AgentMessage::Assistant {
                stop_reason: output_stop,
                raw_stop_reason,
                error_message: output_error,
                ..
            } = &mut self.output
            {
                *output_stop = stop_reason;
                *raw_stop_reason = Some(reason.to_owned());
                if error_message.is_some() {
                    *output_error = error_message;
                }
            }
            self.has_finish_reason = true;
        }
        let Some(delta) = choice.get("delta") else {
            return events;
        };
        if let Some(text) = delta.get("content").and_then(Value::as_str)
            && !text.is_empty()
        {
            let content_index = match self.text_index {
                Some(existing) => existing,
                None => {
                    let content_index = self.content_mut().len();
                    self.content_mut().push(Content::Text {
                        text: String::new(),
                        text_signature: None,
                    });
                    self.text_index = Some(content_index);
                    events.push(AssistantMessageEvent::TextStart {
                        content_index,
                        partial: partial(self),
                    });
                    content_index
                }
            };
            if let Some(Content::Text { text: existing, .. }) =
                self.content_mut().get_mut(content_index)
            {
                existing.push_str(text);
            }
            events.push(AssistantMessageEvent::TextDelta {
                content_index,
                delta: text.to_owned(),
                partial: partial(self),
            });
        }
        for field in REASONING_FIELDS {
            let Some(text) = delta.get(field).and_then(Value::as_str) else {
                continue;
            };
            if text.is_empty() {
                continue;
            }
            let content_index = match self.thinking_index {
                Some(existing) => existing,
                None => {
                    let content_index = self.content_mut().len();
                    self.content_mut().push(Content::Thinking {
                        thinking: String::new(),
                        thinking_signature: Some(field.to_owned()),
                        redacted: None,
                    });
                    self.thinking_index = Some(content_index);
                    events.push(AssistantMessageEvent::ThinkingStart {
                        content_index,
                        partial: partial(self),
                    });
                    content_index
                }
            };
            if let Some(Content::Thinking { thinking, .. }) =
                self.content_mut().get_mut(content_index)
            {
                thinking.push_str(text);
            }
            events.push(AssistantMessageEvent::ThinkingDelta {
                content_index,
                delta: text.to_owned(),
                partial: partial(self),
            });
            break;
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for tool_call in tool_calls {
                let stream_index = tool_call.get("index").and_then(Value::as_u64);
                let id = tool_call.get("id").and_then(Value::as_str).unwrap_or("");
                let name = tool_call
                    .get("function")
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let slot_position = self
                    .tools
                    .iter()
                    .position(|slot| {
                        (stream_index.is_some() && slot.stream_index == stream_index)
                            || (!id.is_empty() && slot.id == id)
                    })
                    .unwrap_or_else(|| {
                        let content_index = self.content_mut().len();
                        self.content_mut().push(Content::ToolCall {
                            id: id.to_owned(),
                            name: name.to_owned(),
                            arguments: serde_json::Map::new(),
                            thought_signature: None,
                            namespace: None,
                        });
                        self.tools.push(ToolSlot {
                            content_index,
                            stream_index,
                            id: id.to_owned(),
                            partial_args: String::new(),
                        });
                        events.push(AssistantMessageEvent::ToolCallStart {
                            content_index,
                            partial: self.output.clone(),
                        });
                        self.tools.len().saturating_sub(1)
                    });
                let (content_index, parsed, delta_text) = {
                    let Some(slot) = self.tools.get_mut(slot_position) else {
                        continue;
                    };
                    if slot.id.is_empty() && !id.is_empty() {
                        slot.id = id.to_owned();
                    }
                    let arguments = tool_call
                        .get("function")
                        .and_then(|function| function.get("arguments"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    slot.partial_args.push_str(arguments);
                    (
                        slot.content_index,
                        parse_streaming_json(&slot.partial_args),
                        arguments.to_owned(),
                    )
                };
                if let Some(Content::ToolCall {
                    id: block_id,
                    name: block_name,
                    arguments,
                    ..
                }) = self.content_mut().get_mut(content_index)
                {
                    if block_id.is_empty() && !id.is_empty() {
                        *block_id = id.to_owned();
                    }
                    if block_name.is_empty() && !name.is_empty() {
                        *block_name = name.to_owned();
                    }
                    *arguments = parsed;
                }
                events.push(AssistantMessageEvent::ToolCallDelta {
                    content_index,
                    delta: delta_text,
                    partial: partial(self),
                });
            }
        }
        events
    }

    pub fn finish(mut self) -> Vec<AssistantMessageEvent> {
        let mut events = Vec::new();
        let content = self.content_mut().clone();
        for (content_index, block) in content.iter().enumerate() {
            match block {
                Content::Text { text, .. } => events.push(AssistantMessageEvent::TextEnd {
                    content_index,
                    content: text.clone(),
                    partial: self.output.clone(),
                }),
                Content::Thinking { thinking, .. } => {
                    events.push(AssistantMessageEvent::ThinkingEnd {
                        content_index,
                        content: thinking.clone(),
                        partial: self.output.clone(),
                    });
                }
                Content::ToolCall { .. } => events.push(AssistantMessageEvent::ToolCallEnd {
                    content_index,
                    tool_call: block.clone(),
                    partial: self.output.clone(),
                }),
                Content::Image { .. } => {}
            }
        }
        let pending = matches!(
            &self.output,
            AgentMessage::Assistant {
                stop_reason: StopReason::Pending,
                ..
            }
        );
        if !self.has_finish_reason || pending {
            events.push(self.fail("Stream ended without finish_reason"));
            return events;
        }
        events.push(crate::request::terminal_event(self.output));
        events
    }

    pub fn fail(&mut self, message: &str) -> AssistantMessageEvent {
        crate::request::fail_message(&mut self.output, message)
    }
}

fn run_request(
    model: &Model,
    body: &Value,
    api_key: &str,
    sender: &Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    let url = format!("{}/chat/completions", model.base_url);
    let headers = [("authorization", format!("Bearer {api_key}"))];
    let response = crate::request::send_with_retry(&url, &headers, body)?;
    let mut mapper = ChunkMapper::new(model);
    let _ = sender.blocking_send(mapper.start_event());
    crate::request::pump_sse(response, |sse| {
        if sse.data == "[DONE]" {
            return Ok(());
        }
        if let Ok(payload) = parse_json_with_repair(&sse.data) {
            for event in mapper.push_chunk(&payload) {
                let _ = sender.blocking_send(event);
            }
        }
        Ok(())
    })?;
    for event in mapper.finish() {
        let _ = sender.blocking_send(event);
    }
    Ok(())
}

pub fn stream(
    model: &Model,
    context: &LlmContext,
    options: &OpenAiOptions,
    api_key: &str,
) -> Receiver<AssistantMessageEvent> {
    let (sender, receiver) = tokio::sync::mpsc::channel(256);
    let body = build_params(model, context, options);
    let model = model.clone();
    let api_key = api_key.to_owned();
    tokio::task::spawn_blocking(move || {
        if let Err(message) = run_request(&model, &body, &api_key, &sender) {
            let mut mapper = ChunkMapper::new(&model);
            let event = mapper.fail(&message);
            let _ = sender.blocking_send(event);
        }
    });
    receiver
}
