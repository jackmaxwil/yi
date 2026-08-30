use serde_json::{Map, Value, json};
use tokio::sync::mpsc::{Receiver, Sender};
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, Content, StopReason, Usage, UserContent};
use yi_types::model::{LlmContext, Model, SYSTEM_BLOCK_SEPARATOR, ToolDef};

use crate::catalog::calculate_cost;
use crate::compat::compat_bool;
use crate::json_salvage::{parse_json_with_repair, parse_streaming_json};
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
    /// Interactive sessions hold the stable prefix for an hour.
    pub cache_1h: bool,
    pub proxy: Option<crate::request::ProxyConfig>,
}

const CACHE_TTL_BETA: &str = "extended-cache-ttl-2025-04-11";

fn ephemeral(long: bool) -> Value {
    if long {
        json!({"type": "ephemeral", "ttl": "1h"})
    } else {
        json!({"type": "ephemeral"})
    }
}

/// Three of the four breakpoints: universal prefix, trusted prompt, yard. The
/// fourth is the newest message.
fn system_blocks(system: &str, options: &AnthropicOptions) -> Vec<Value> {
    let parts: Vec<&str> = system.split(SYSTEM_BLOCK_SEPARATOR).collect();
    let mut texts: Vec<String> = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        if index < 3 {
            texts.push((*part).to_owned());
        } else if let Some(last) = texts.last_mut() {
            last.push_str("\n\n");
            last.push_str(part);
        }
    }
    texts
        .iter()
        .map(|text| {
            let mut block = json!({"type": "text", "text": text});
            if options.cache {
                block["cache_control"] = ephemeral(options.cache_1h);
            }
            block
        })
        .collect()
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
                        // D51: one shape every turn. A bare string reshapes
                        // when it carries the breakpoint, moving the prefix
                        // hash and re-billing the whole history.
                        Value::Array(vec![text_block(text)])
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
        && let Value::Array(blocks) = &mut last["content"]
        && let Some(block) = blocks.last_mut()
    {
        block["cache_control"] = json!({"type": "ephemeral"});
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
        params["system"] = Value::Array(system_blocks(&context.system_prompt, options));
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
        // Tools precede system, so a system breakpoint already caches them.
        let cache_tools = options.cache && context.system_prompt.is_empty();
        params["tools"] = Value::Array(convert_tools(tools, cache_tools));
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
            output: crate::request::empty_assistant(model),
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

    fn partial(&self) -> AgentMessage {
        self.output.clone()
    }

    pub fn push(&mut self, payload: &Value) -> Vec<AssistantMessageEvent> {
        let mut events = Vec::new();
        match payload.get("type").and_then(Value::as_str) {
            Some("message_start") => self.on_message_start(payload),
            Some("content_block_start") => self.on_block_start(payload, &mut events),
            Some("content_block_delta") => self.on_block_delta(payload, &mut events),
            Some("content_block_stop") => self.on_block_stop(payload, &mut events),
            Some("message_delta") => self.on_message_delta(payload),
            Some("message_stop") => {
                self.finished = true;
            }
            _ => {}
        }
        events
    }

    fn on_message_start(&mut self, payload: &Value) {
        {
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
            let read = |key: &str| usage_value.get(key).and_then(Value::as_i64).unwrap_or(0);
            {
                let usage = self.usage_mut();
                usage.input = read("input_tokens");
                usage.output = read("output_tokens");
                usage.cache_read = read("cache_read_input_tokens");
                usage.cache_write = read("cache_creation_input_tokens");
                let write1h = usage_value
                    .get("cache_creation")
                    .and_then(|creation| creation.get("ephemeral_1h_input_tokens"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                usage.cache_write1h = Some(write1h);
                usage.unknown = usage_value.is_null();
            }
        }
        self.recompute_totals();
    }

    fn on_block_start(&mut self, payload: &Value, events: &mut Vec<AssistantMessageEvent>) {
        {
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
                        partial: self.partial(),
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
                        partial: self.partial(),
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
                        partial: self.partial(),
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
                        partial: self.partial(),
                    });
                }
                _ => {
                    self.partial_json.push(None);
                    self.api_indices.push(api_index);
                }
            }
        }
    }

    fn on_block_delta(&mut self, payload: &Value, events: &mut Vec<AssistantMessageEvent>) {
        {
            let api_index = payload.get("index").and_then(Value::as_u64).unwrap_or(0);
            let Some(content_index) = self.local_index(api_index) else {
                return;
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
                        partial: self.partial(),
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
                        partial: self.partial(),
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
                        partial: self.partial(),
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
    }

    fn on_block_stop(&mut self, payload: &Value, events: &mut Vec<AssistantMessageEvent>) {
        {
            let api_index = payload.get("index").and_then(Value::as_u64).unwrap_or(0);
            let Some(content_index) = self.local_index(api_index) else {
                return;
            };
            let block = self.content_mut().get(content_index).cloned();
            match block {
                Some(Content::Text { text, .. }) => {
                    events.push(AssistantMessageEvent::TextEnd {
                        content_index,
                        content: text,
                        partial: self.partial(),
                    });
                }
                Some(Content::Thinking { thinking, .. }) => {
                    events.push(AssistantMessageEvent::ThinkingEnd {
                        content_index,
                        content: thinking,
                        partial: self.partial(),
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
                            partial: self.partial(),
                        });
                    }
                }
                _ => {}
            }
        }
    }

    fn on_message_delta(&mut self, payload: &Value) {
        {
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
            if let Some(usage_value) = payload.get("usage").filter(|value| !value.is_null()) {
                let usage = self.usage_mut();
                usage.unknown = false;
                let update = |target: &mut i64, key: &str| {
                    if let Some(value) = usage_value.get(key).and_then(Value::as_i64) {
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
                    .and_then(Value::as_i64)
                {
                    usage.reasoning = Some(reasoning);
                }
            }
        }
        self.recompute_totals();
    }

    pub fn finish(mut self) -> AssistantMessageEvent {
        let pending = matches!(
            &self.output,
            AgentMessage::Assistant {
                stop_reason: StopReason::Pending,
                ..
            }
        );
        if !self.finished || pending {
            return self.fail("Anthropic stream ended without a stop reason");
        }
        crate::request::terminal_event(self.output)
    }

    pub fn fail(&mut self, message: &str) -> AssistantMessageEvent {
        crate::request::fail_message(&mut self.output, message)
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
    long_cache: bool,
    proxy: Option<&crate::request::ProxyConfig>,
    sender: &Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    let url = format!("{}/v1/messages", model.base_url);
    let mut headers = vec![
        ("x-api-key", api_key.to_owned()),
        ("anthropic-version", ANTHROPIC_VERSION.to_owned()),
    ];
    if long_cache {
        headers.push(("anthropic-beta", CACHE_TTL_BETA.to_owned()));
    }
    let response = crate::request::send_with_retry(&url, &headers, body, proxy)?;
    let mut mapper = Mapper::new(model);
    let _ = sender.blocking_send(mapper.start_event());
    crate::request::pump_sse(response, |sse| {
        let kind = sse.event.as_deref().unwrap_or("");
        if kind == "error" {
            return Err(sse.data);
        }
        if !ANTHROPIC_MESSAGE_EVENTS.contains(&kind) {
            return Ok(true);
        }
        let payload = parse_json_with_repair(&sse.data)
            .map_err(|error| format!("Could not parse Anthropic SSE event {kind}: {error}"))?;
        for event in mapper.push(&payload) {
            let _ = sender.blocking_send(event);
        }
        Ok(true)
    })?;
    let _ = sender.blocking_send(mapper.finish());
    Ok(())
}

/// An hour of cache retention is a beta on some accounts and models. If the
/// API refuses it, the request is retried once at the default five minutes:
/// a shorter cache is a cost, a failed turn is an outage.
pub fn is_cache_retention_rejection(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.starts_with("http 400") && (lower.contains("ttl") || lower.contains("beta"))
}

pub fn drop_cache_retention(body: &mut Value) {
    let Some(blocks) = body.get_mut("system").and_then(Value::as_array_mut) else {
        return;
    };
    for block in blocks {
        if let Some(control) = block
            .get_mut("cache_control")
            .and_then(Value::as_object_mut)
        {
            control.remove("ttl");
        }
    }
}

pub fn stream(
    model: &Model,
    context: &LlmContext,
    options: &AnthropicOptions,
    api_key: &str,
) -> Receiver<AssistantMessageEvent> {
    let (sender, receiver) = tokio::sync::mpsc::channel(256);
    let mut body = build_params(model, context, options);
    let model = model.clone();
    let api_key = api_key.to_owned();
    let proxy = options.proxy.clone();
    let long_cache = options.cache && options.cache_1h;
    tokio::task::spawn_blocking(move || {
        let Err(message) =
            run_request(&model, &body, &api_key, long_cache, proxy.as_ref(), &sender)
        else {
            return;
        };
        if long_cache && is_cache_retention_rejection(&message) {
            drop_cache_retention(&mut body);
            if run_request(&model, &body, &api_key, false, proxy.as_ref(), &sender).is_ok() {
                return;
            }
        }
        let mut mapper = Mapper::new(&model);
        let event = mapper.fail(&message);
        let _ = sender.blocking_send(event);
    });
    receiver
}
