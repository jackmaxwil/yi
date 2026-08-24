use serde_json::{Map, Value, json};
use tokio::sync::mpsc::{Receiver, Sender};
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, Content, StopReason, Usage, UserContent};
use yi_types::model::{LlmContext, Model, ToolDef};

use crate::catalog::calculate_cost;
use crate::json_salvage::{parse_json_with_repair, parse_streaming_json};
use crate::retry::{RetryPolicy, is_retryable_status, retry_delay};
use crate::sse::SseDecoder;
use crate::transform::{normalize_anthropic_tool_call_id, transform_messages};

pub const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, Default, PartialEq)]
pub enum Thinking {
    #[default]
    Off,
    Adaptive {
        effort: Option<String>,
    },
    Budget {
        tokens: u64,
    },
}

#[derive(Debug, Clone, Default)]
pub struct AnthropicOptions {
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub thinking: Thinking,
    pub cache: bool,
}

fn compat_bool(model: &Model, key: &str, default: bool) -> bool {
    model
        .compat
        .as_ref()
        .and_then(|compat| compat.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn text_block(text: &str) -> Value {
    json!({"type": "text", "text": text})
}

fn image_block(data: &str, mime_type: &str) -> Value {
    json!({"type": "image", "source": {"type": "base64", "media_type": mime_type, "data": data}})
}

fn content_blocks(content: &[Content]) -> Value {
    let has_images = content
        .iter()
        .any(|block| matches!(block, Content::Image { .. }));
    if !has_images {
        let joined = content
            .iter()
            .filter_map(|block| match block {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        return Value::String(joined);
    }
    let mut blocks: Vec<Value> = content
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text_block(text)),
            Content::Image { data, mime_type } => Some(image_block(data, mime_type)),
            _ => None,
        })
        .collect();
    if !blocks.iter().any(|block| block["type"] == "text") {
        blocks.insert(0, text_block("(see attached image)"));
    }
    Value::Array(blocks)
}

fn convert_messages(messages: &[AgentMessage], cache: bool) -> Vec<Value> {
    let mut params: Vec<Value> = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        match &messages[index] {
            AgentMessage::User { content, .. } => {
                let value = match content {
                    UserContent::Text(text) => {
                        if text.trim().is_empty() {
                            index += 1;
                            continue;
                        }
                        Value::String(text.clone())
                    }
                    UserContent::Blocks(blocks) => {
                        let converted: Vec<Value> = blocks
                            .iter()
                            .filter_map(|block| match block {
                                Content::Text { text, .. } => {
                                    (!text.trim().is_empty()).then(|| text_block(text))
                                }
                                Content::Image { data, mime_type } => {
                                    Some(image_block(data, mime_type))
                                }
                                _ => None,
                            })
                            .collect();
                        if converted.is_empty() {
                            index += 1;
                            continue;
                        }
                        Value::Array(converted)
                    }
                };
                params.push(json!({"role": "user", "content": value}));
            }
            AgentMessage::Assistant { content, .. } => {
                let mut blocks: Vec<Value> = Vec::new();
                for block in content {
                    match block {
                        Content::Text { text, .. } => {
                            if !text.trim().is_empty() {
                                blocks.push(text_block(text));
                            }
                        }
                        Content::Thinking {
                            thinking,
                            thinking_signature,
                            redacted,
                        } => {
                            if redacted == &Some(true) {
                                if let Some(signature) = thinking_signature {
                                    blocks.push(
                                        json!({"type": "redacted_thinking", "data": signature}),
                                    );
                                }
                                continue;
                            }
                            let has_signature = thinking_signature
                                .as_deref()
                                .is_some_and(|signature| !signature.trim().is_empty());
                            if thinking.trim().is_empty() && !has_signature {
                                continue;
                            }
                            if has_signature {
                                blocks.push(json!({
                                    "type": "thinking",
                                    "thinking": thinking,
                                    "signature": thinking_signature,
                                }));
                            } else {
                                blocks.push(text_block(thinking));
                            }
                        }
                        Content::ToolCall {
                            id,
                            name,
                            arguments,
                            ..
                        } => {
                            blocks.push(json!({
                                "type": "tool_use",
                                "id": id,
                                "name": name,
                                "input": Value::Object(arguments.clone()),
                            }));
                        }
                        Content::Image { .. } => {}
                    }
                }
                if !blocks.is_empty() {
                    params.push(json!({"role": "assistant", "content": blocks}));
                }
            }
            AgentMessage::ToolResult { .. } => {
                let mut results: Vec<Value> = Vec::new();
                while let Some(AgentMessage::ToolResult {
                    tool_call_id,
                    content,
                    is_error,
                    ..
                }) = messages.get(index)
                {
                    results.push(json!({
                        "type": "tool_result",
                        "tool_use_id": tool_call_id,
                        "content": content_blocks(content),
                        "is_error": is_error,
                    }));
                    index += 1;
                }
                index -= 1;
                params.push(json!({"role": "user", "content": results}));
            }
            _ => {}
        }
        index += 1;
    }

    if cache
        && let Some(last) = params.last_mut()
        && last["role"] == "user"
    {
        match &mut last["content"] {
            Value::Array(blocks) => {
                if let Some(block) = blocks.last_mut() {
                    block["cache_control"] = json!({"type": "ephemeral"});
                }
            }
            Value::String(text) => {
                let text = text.clone();
                last["content"] = json!([{
                    "type": "text",
                    "text": text,
                    "cache_control": {"type": "ephemeral"},
                }]);
            }
            _ => {}
        }
    }
    params
}

fn convert_tools(tools: &[ToolDef], cache: bool) -> Vec<Value> {
    tools
        .iter()
        .enumerate()
        .map(|(index, tool)| {
            let schema = &tool.parameters;
            let mut value = json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": {
                    "type": "object",
                    "properties": schema.get("properties").cloned().unwrap_or_else(|| json!({})),
                    "required": schema.get("required").cloned().unwrap_or_else(|| json!([])),
                },
            });
            if cache && index == tools.len().saturating_sub(1) {
                value["cache_control"] = json!({"type": "ephemeral"});
            }
            value
        })
        .collect()
}

pub fn build_params(model: &Model, context: &LlmContext, options: &AnthropicOptions) -> Value {
    let transformed = transform_messages(
        &context.messages,
        model,
        Some(normalize_anthropic_tool_call_id),
    );
    let mut params = json!({
        "model": model.id,
        "messages": convert_messages(&transformed, options.cache),
        "max_tokens": options.max_tokens.unwrap_or(model.max_tokens),
        "stream": true,
    });
    if !context.system_prompt.is_empty() {
        let mut system = json!({"type": "text", "text": context.system_prompt});
        if options.cache {
            system["cache_control"] = json!({"type": "ephemeral"});
        }
        params["system"] = json!([system]);
    }
    let supports_temperature = compat_bool(model, "supportsTemperature", true);
    if let Some(temperature) = options.temperature
        && options.thinking == Thinking::Off
        && supports_temperature
    {
        params["temperature"] = json!(temperature);
    }
    if let Some(tools) = &context.tools
        && !tools.is_empty()
    {
        params["tools"] = Value::Array(convert_tools(tools, options.cache));
    }
    if model.reasoning {
        match &options.thinking {
            Thinking::Adaptive { effort } => {
                params["thinking"] = json!({"type": "adaptive", "display": "summarized"});
                if let Some(effort) = effort {
                    params["output_config"] = json!({"effort": effort});
                }
            }
            Thinking::Budget { tokens } => {
                params["thinking"] = json!({
                    "type": "enabled",
                    "budget_tokens": if *tokens == 0 { 1024 } else { *tokens },
                    "display": "summarized",
                });
            }
            Thinking::Off => {
                let off_is_null = model
                    .thinking_level_map
                    .as_ref()
                    .and_then(|map| map.get("off"))
                    .is_some_and(Value::is_null);
                if !off_is_null {
                    params["thinking"] = json!({"type": "disabled"});
                }
            }
        }
    }
    params
}

fn map_stop_reason(reason: &str, stop_details: Option<&Value>) -> (StopReason, Option<String>) {
    match reason {
        "end_turn" | "pause_turn" | "stop_sequence" => (StopReason::Stop, None),
        "max_tokens" => (StopReason::Length, None),
        "tool_use" => (StopReason::ToolUse, None),
        "refusal" => (
            StopReason::Error,
            Some(
                stop_details
                    .and_then(|details| details.get("explanation"))
                    .and_then(Value::as_str)
                    .unwrap_or("The model refused to complete the request")
                    .to_owned(),
            ),
        ),
        other => (
            StopReason::Error,
            Some(format!("Provider stopped with: {other}")),
        ),
    }
}

/// Streaming state machine: consumes parsed Anthropic SSE payloads, builds the
/// assistant message, and yields pi-ai shaped events.
pub struct Mapper {
    output: AgentMessage,
    partial_json: Vec<Option<String>>,
    api_indices: Vec<u64>,
    model: Model,
    finished: bool,
}

impl Mapper {
    pub fn new(model: &Model) -> Self {
        Self {
            output: AgentMessage::Assistant {
                content: Vec::new(),
                api: model.api.clone(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage: Usage::zero(),
                stop_reason: StopReason::Pending,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: 0,
            },
            partial_json: Vec::new(),
            api_indices: Vec::new(),
            model: model.clone(),
            finished: false,
        }
    }

    fn content_mut(&mut self) -> &mut Vec<Content> {
        match &mut self.output {
            AgentMessage::Assistant { content, .. } => content,
            _ => unreachable!("mapper output is always an assistant message"),
        }
    }

    fn usage_mut(&mut self) -> &mut Usage {
        match &mut self.output {
            AgentMessage::Assistant { usage, .. } => usage,
            _ => unreachable!("mapper output is always an assistant message"),
        }
    }

    fn local_index(&self, api_index: u64) -> Option<usize> {
        self.api_indices
            .iter()
            .position(|index| *index == api_index)
    }

    fn recompute_totals(&mut self) {
        let usage = self.usage_mut();
        usage.total_tokens = usage
            .input
            .saturating_add(usage.output)
            .saturating_add(usage.cache_read)
            .saturating_add(usage.cache_write);
        let model = self.model.clone();
        calculate_cost(&model, self.usage_mut());
    }

    pub fn push(&mut self, payload: &Value) -> Vec<AssistantMessageEvent> {
        let mut events = Vec::new();
        let partial = |mapper: &Self| mapper.output.clone();
        match payload.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                let message = &payload["message"];
                if let AgentMessage::Assistant {
                    response_id, model, ..
                } = &mut self.output
                {
                    if let Some(id) = message.get("id").and_then(Value::as_str) {
                        *response_id = Some(id.to_owned());
                    }
                    if let Some(response_model) = message.get("model").and_then(Value::as_str) {
                        *model = response_model.to_owned();
                    }
                }
                let usage_value = &message["usage"];
                let read = |key: &str| usage_value.get(key).and_then(Value::as_u64).unwrap_or(0);
                {
                    let usage = self.usage_mut();
                    usage.input = read("input_tokens");
                    usage.output = read("output_tokens");
                    usage.cache_read = read("cache_read_input_tokens");
                    usage.cache_write = read("cache_creation_input_tokens");
                    let write1h = usage_value
                        .get("cache_creation")
                        .and_then(|creation| creation.get("ephemeral_1h_input_tokens"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    usage.cache_write1h = Some(write1h);
                }
                self.recompute_totals();
            }
            Some("content_block_start") => {
                let api_index = payload.get("index").and_then(Value::as_u64).unwrap_or(0);
                let block = &payload["content_block"];
                let content_index = self.content_mut().len();
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                        self.content_mut().push(Content::Text {
                            text: text.to_owned(),
                            text_signature: None,
                        });
                        self.partial_json.push(None);
                        self.api_indices.push(api_index);
                        events.push(AssistantMessageEvent::TextStart {
                            content_index,
                            partial: partial(self),
                        });
                    }
                    Some("thinking") => {
                        self.content_mut().push(Content::Thinking {
                            thinking: block
                                .get("thinking")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned(),
                            thinking_signature: Some(
                                block
                                    .get("signature")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_owned(),
                            ),
                            redacted: None,
                        });
                        self.partial_json.push(None);
                        self.api_indices.push(api_index);
                        events.push(AssistantMessageEvent::ThinkingStart {
                            content_index,
                            partial: partial(self),
                        });
                    }
                    Some("redacted_thinking") => {
                        self.content_mut().push(Content::Thinking {
                            thinking: "[Reasoning redacted]".to_owned(),
                            thinking_signature: block
                                .get("data")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            redacted: Some(true),
                        });
                        self.partial_json.push(None);
                        self.api_indices.push(api_index);
                        events.push(AssistantMessageEvent::ThinkingStart {
                            content_index,
                            partial: partial(self),
                        });
                    }
                    Some("tool_use") => {
                        let arguments = block
                            .get("input")
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        self.content_mut().push(Content::ToolCall {
                            id: block
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned(),
                            name: block
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned(),
                            arguments,
                            thought_signature: None,
                            namespace: None,
                        });
                        self.partial_json.push(Some(String::new()));
                        self.api_indices.push(api_index);
                        events.push(AssistantMessageEvent::ToolCallStart {
                            content_index,
                            partial: partial(self),
                        });
                    }
                    _ => {
                        self.partial_json.push(None);
                        self.api_indices.push(api_index);
                    }
                }
            }
            Some("content_block_delta") => {
                let api_index = payload.get("index").and_then(Value::as_u64).unwrap_or(0);
                let Some(content_index) = self.local_index(api_index) else {
                    return events;
                };
                let delta = &payload["delta"];
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        let chunk = delta.get("text").and_then(Value::as_str).unwrap_or("");
                        if let Some(Content::Text { text, .. }) =
                            self.content_mut().get_mut(content_index)
                        {
                            text.push_str(chunk);
                        }
                        events.push(AssistantMessageEvent::TextDelta {
                            content_index,
                            delta: chunk.to_owned(),
                            partial: partial(self),
                        });
                    }
                    Some("thinking_delta") => {
                        let chunk = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
                        if let Some(Content::Thinking { thinking, .. }) =
                            self.content_mut().get_mut(content_index)
                        {
                            thinking.push_str(chunk);
                        }
                        events.push(AssistantMessageEvent::ThinkingDelta {
                            content_index,
                            delta: chunk.to_owned(),
                            partial: partial(self),
                        });
                    }
                    Some("input_json_delta") => {
                        let chunk = delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let parsed =
                            if let Some(Some(buffer)) = self.partial_json.get_mut(content_index) {
                                buffer.push_str(chunk);
                                parse_streaming_json(buffer)
                            } else {
                                Map::new()
                            };
                        if let Some(Content::ToolCall { arguments, .. }) =
                            self.content_mut().get_mut(content_index)
                        {
                            *arguments = parsed;
                        }
                        events.push(AssistantMessageEvent::ToolCallDelta {
                            content_index,
                            delta: chunk.to_owned(),
                            partial: partial(self),
                        });
                    }
                    Some("signature_delta") => {
                        let chunk = delta.get("signature").and_then(Value::as_str).unwrap_or("");
                        if let Some(Content::Thinking {
                            thinking_signature, ..
                        }) = self.content_mut().get_mut(content_index)
                        {
                            match thinking_signature {
                                Some(signature) => signature.push_str(chunk),
                                None => *thinking_signature = Some(chunk.to_owned()),
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                let api_index = payload.get("index").and_then(Value::as_u64).unwrap_or(0);
                let Some(content_index) = self.local_index(api_index) else {
                    return events;
                };
                let block = self.content_mut().get(content_index).cloned();
                match block {
                    Some(Content::Text { text, .. }) => {
                        events.push(AssistantMessageEvent::TextEnd {
                            content_index,
                            content: text,
                            partial: partial(self),
                        });
                    }
                    Some(Content::Thinking { thinking, .. }) => {
                        events.push(AssistantMessageEvent::ThinkingEnd {
                            content_index,
                            content: thinking,
                            partial: partial(self),
                        });
                    }
                    Some(Content::ToolCall { .. }) => {
                        let final_arguments = self
                            .partial_json
                            .get(content_index)
                            .and_then(|buffer| buffer.as_deref())
                            .map(parse_streaming_json)
                            .unwrap_or_default();
                        if let Some(Content::ToolCall { arguments, .. }) =
                            self.content_mut().get_mut(content_index)
                            && (!final_arguments.is_empty() || arguments.is_empty())
                        {
                            *arguments = final_arguments;
                        }
                        if let Some(block) = self.content_mut().get(content_index).cloned() {
                            events.push(AssistantMessageEvent::ToolCallEnd {
                                content_index,
                                tool_call: block,
                                partial: partial(self),
                            });
                        }
                    }
                    _ => {}
                }
            }
            Some("message_delta") => {
                if let Some(reason) = payload
                    .get("delta")
                    .and_then(|delta| delta.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    let details = payload
                        .get("delta")
                        .and_then(|delta| delta.get("stop_details"));
                    let (stop_reason, error_message) = map_stop_reason(reason, details);
                    if let AgentMessage::Assistant {
                        stop_reason: message_stop,
                        raw_stop_reason,
                        error_message: message_error,
                        ..
                    } = &mut self.output
                    {
                        *message_stop = stop_reason;
                        *raw_stop_reason = Some(reason.to_owned());
                        if error_message.is_some() {
                            *message_error = error_message;
                        }
                    }
                }
                if let Some(usage_value) = payload.get("usage") {
                    let usage = self.usage_mut();
                    let update = |target: &mut u64, key: &str| {
                        if let Some(value) = usage_value.get(key).and_then(Value::as_u64) {
                            *target = value;
                        }
                    };
                    update(&mut usage.input, "input_tokens");
                    update(&mut usage.output, "output_tokens");
                    update(&mut usage.cache_read, "cache_read_input_tokens");
                    update(&mut usage.cache_write, "cache_creation_input_tokens");
                    if let Some(reasoning) = usage_value
                        .get("output_tokens_details")
                        .and_then(|details| details.get("thinking_tokens"))
                        .and_then(Value::as_u64)
                    {
                        usage.reasoning = Some(reasoning);
                    }
                }
                self.recompute_totals();
            }
            Some("message_stop") => {
                self.finished = true;
            }
            _ => {}
        }
        events
    }

    pub fn finish(mut self) -> AssistantMessageEvent {
        let stop_reason = match &self.output {
            AgentMessage::Assistant { stop_reason, .. } => *stop_reason,
            _ => StopReason::Error,
        };
        if !self.finished || stop_reason == StopReason::Pending {
            return self.fail("Anthropic stream ended without a stop reason");
        }
        if stop_reason == StopReason::Error || stop_reason == StopReason::Aborted {
            return AssistantMessageEvent::Error {
                reason: stop_reason,
                error: self.output,
            };
        }
        AssistantMessageEvent::Done {
            reason: stop_reason,
            message: self.output,
        }
    }

    pub fn fail(&mut self, message: &str) -> AssistantMessageEvent {
        if let AgentMessage::Assistant {
            stop_reason,
            error_message,
            ..
        } = &mut self.output
        {
            *stop_reason = StopReason::Error;
            *error_message = Some(message.to_owned());
        }
        AssistantMessageEvent::Error {
            reason: StopReason::Error,
            error: self.output.clone(),
        }
    }

    pub fn start_event(&self) -> AssistantMessageEvent {
        AssistantMessageEvent::Start {
            partial: self.output.clone(),
        }
    }
}

const ANTHROPIC_MESSAGE_EVENTS: [&str; 6] = [
    "message_start",
    "message_delta",
    "message_stop",
    "content_block_start",
    "content_block_delta",
    "content_block_stop",
];

fn run_request(
    model: &Model,
    body: &Value,
    api_key: &str,
    sender: &Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    let policy = RetryPolicy::default();
    let started = std::time::Instant::now();
    let url = format!("{}/v1/messages", model.base_url);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(30))
        .timeout_read(std::time::Duration::from_secs(60))
        .build();
    let mut attempt: u32 = 0;
    let response = loop {
        let request = agent
            .post(&url)
            .set("x-api-key", api_key)
            .set("anthropic-version", ANTHROPIC_VERSION)
            .set("accept", "application/json")
            .set("content-type", "application/json");
        match request.send_string(&body.to_string()) {
            Ok(response) => break response,
            Err(ureq::Error::Status(status, response)) => {
                let retryable = is_retryable_status(status)
                    && attempt < policy.max_attempts
                    && started.elapsed() < policy.max_total_wall;
                if !retryable {
                    let text = response.into_string().unwrap_or_default();
                    return Err(format!("HTTP {status}: {text}"));
                }
                let retry_after_ms = response
                    .header("retry-after-ms")
                    .and_then(|value| value.parse::<f64>().ok());
                let retry_after = response
                    .header("retry-after")
                    .and_then(|value| value.parse::<f64>().ok());
                if let Some(delay) = retry_delay(attempt, retry_after_ms, retry_after, &policy) {
                    std::thread::sleep(delay);
                }
                attempt = attempt.saturating_add(1);
            }
            Err(error) => {
                if attempt < policy.max_attempts && started.elapsed() < policy.max_total_wall {
                    if let Some(delay) = retry_delay(attempt, None, None, &policy) {
                        std::thread::sleep(delay);
                    }
                    attempt = attempt.saturating_add(1);
                    continue;
                }
                return Err(error.to_string());
            }
        }
    };

    let mut mapper = Mapper::new(model);
    let _ = sender.blocking_send(mapper.start_event());
    let mut reader = response.into_reader();
    let mut decoder = SseDecoder::default();
    let mut buffer = [0u8; 8192];
    let mut pending = Vec::new();
    loop {
        let read =
            std::io::Read::read(&mut reader, &mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        pending.extend_from_slice(buffer.get(..read).unwrap_or_default());
        let Ok(chunk) = std::str::from_utf8(&pending) else {
            continue;
        };
        let chunk = chunk.to_owned();
        pending.clear();
        for sse in decoder.feed(&chunk) {
            let kind = sse.event.as_deref().unwrap_or("");
            if kind == "error" {
                return Err(sse.data);
            }
            if !ANTHROPIC_MESSAGE_EVENTS.contains(&kind) {
                continue;
            }
            let payload = parse_json_with_repair(&sse.data)
                .map_err(|error| format!("Could not parse Anthropic SSE event {kind}: {error}"))?;
            for event in mapper.push(&payload) {
                let _ = sender.blocking_send(event);
            }
        }
    }
    for sse in decoder.finish() {
        if let Ok(payload) = parse_json_with_repair(&sse.data) {
            for event in mapper.push(&payload) {
                let _ = sender.blocking_send(event);
            }
        }
    }
    let _ = sender.blocking_send(mapper.finish());
    Ok(())
}

pub fn stream(
    model: &Model,
    context: &LlmContext,
    options: &AnthropicOptions,
    api_key: &str,
) -> Receiver<AssistantMessageEvent> {
    let (sender, receiver) = tokio::sync::mpsc::channel(256);
    let body = build_params(model, context, options);
    let model = model.clone();
    let api_key = api_key.to_owned();
    tokio::task::spawn_blocking(move || {
        if let Err(message) = run_request(&model, &body, &api_key, &sender) {
            let mut mapper = Mapper::new(&model);
            let event = mapper.fail(&message);
            let _ = sender.blocking_send(event);
        }
    });
    receiver
}
