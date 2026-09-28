use serde_json::{Map, Value, json};
use tokio::sync::mpsc::{Receiver, Sender};
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, Content, StopReason, Usage, UserContent};
use yi_types::model::{LlmContext, Model, SYSTEM_BLOCK_SEPARATOR, ToolChoice, ToolDef};

use crate::breakpoints::{Breakpoints, CacheRoute, Dialect, Reuse, encode};
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
    /// Interactive sessions hold the stable prefix for an hour.
    pub cache_1h: bool,
    pub proxy: Option<crate::request::ProxyConfig>,
    /// The loop's cut of this request (D163): set, the pump stops at the next event.
    pub stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Bearer instead of `x-api-key`: a stored OAuth credential, not a key.
    pub oauth: bool,
    pub extra_headers: Vec<(String, String)>,
}

/// Universal prefix, trusted prompt, yard: one block each, so a later stage can mark a
/// boundary between them; the stable breakpoint sits on the last one (D295).
fn system_blocks(system: &str) -> Vec<Value> {
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
    texts.iter().map(|text| text_block(text)).collect()
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

/// Rendered messages beside, per rendered message, the index of the message it came from,
/// which is what a breakpoint position names.
fn convert_messages(messages: &[AgentMessage]) -> (Vec<Value>, Vec<Option<usize>>) {
    let _span = yi_types::trace::span("ai.convert_messages");
    let mut params: Vec<Value> = Vec::new();
    let mut origins: Vec<Option<usize>> = Vec::new();
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
                        // D51: one shape every turn. A bare string reshapes when it carries
                        // the breakpoint, moving the prefix hash and re-billing the history.
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
                origins.push(Some(index));
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
                    origins.push(Some(index));
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
                origins.push(Some(index));
            }
            _ => {}
        }
        index += 1;
    }
    (params, origins)
}

fn convert_tools(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            let schema = &tool.parameters;
            json!({
                "name": tool.name,
                "description": tool.description,
                "input_schema": {
                    "type": "object",
                    "properties": schema.get("properties").cloned().unwrap_or_else(|| json!({})),
                    "required": schema.get("required").cloned().unwrap_or_else(|| json!([])),
                },
            })
        })
        .collect()
}

fn convert_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!({"type": "auto"}),
        ToolChoice::None => json!({"type": "none"}),
        ToolChoice::Tool(forced) => json!({"type": "tool", "name": forced.as_str()}),
    }
}

pub fn build_params(model: &Model, context: &LlmContext, options: &AnthropicOptions) -> Value {
    let _span = yi_types::trace::span("ai.build_params")
        .arg("api", "anthropic")
        .arg("messages", context.messages.len());
    let history = transform_messages(
        &context.messages,
        model,
        Some(normalize_anthropic_tool_call_id),
    );
    let breakpoints = Breakpoints::build(
        &CacheRoute::of(model, options.cache_1h),
        &history,
        Reuse::of(context),
    );
    let (messages, origins) = convert_messages(&history);
    let mut params = json!({
        "model": model.id,
        "messages": messages,
        "max_tokens": options.max_tokens.unwrap_or(model.max_tokens),
        "stream": true,
    });
    if !context.system_prompt.is_empty() {
        params["system"] = Value::Array(system_blocks(&context.system_prompt));
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
        params["tools"] = Value::Array(convert_tools(tools));
    }
    if let Some(choice) = &context.tool_choice {
        params["tool_choice"] = convert_tool_choice(choice);
    }
    // Incident: the API refuses a forced tool beside extended thinking, so that turn goes
    // without it; the toggle costs one messages-level cache write, tools and system stay hot.
    let forced = matches!(context.tool_choice, Some(ToolChoice::Tool(_)));
    let thinking = if forced {
        &Thinking::Off
    } else {
        &options.thinking
    };
    if model.reasoning {
        match thinking {
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
    encode(
        &breakpoints,
        Dialect::AnthropicBlocks,
        &mut params,
        &origins,
    );
    // The per-request facts render after every mark, so none can land on them.
    let (transient, _) = convert_messages(&transform_messages(
        &context.transient,
        model,
        Some(normalize_anthropic_tool_call_id),
    ));
    if let Some(messages) = params.get_mut("messages").and_then(Value::as_array_mut) {
        messages.extend(transient);
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
/// assistant message, and yields `AssistantMessageEvent`s.
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
                    events.push(AssistantMessageEvent::TextStart { content_index });
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
                    events.push(AssistantMessageEvent::ThinkingStart { content_index });
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
                    events.push(AssistantMessageEvent::ThinkingStart { content_index });
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
                    let name = block.get("name").and_then(Value::as_str).map(str::to_owned);
                    events.push(AssistantMessageEvent::ToolCallStart {
                        content_index,
                        name,
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
                    });
                }
                Some(Content::Thinking { thinking, .. }) => {
                    events.push(AssistantMessageEvent::ThinkingEnd {
                        content_index,
                        content: thinking,
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
    wire: crate::request::Wire<'_>,
    sender: &Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    let crate::request::Wire {
        api_key,
        proxy,
        stop,
        extra,
        oauth,
    } = wire;
    let url = format!("{}/v1/messages", model.base_url);
    let base = if oauth {
        vec![
            ("authorization", format!("Bearer {api_key}")),
            ("anthropic-version", ANTHROPIC_VERSION.to_owned()),
        ]
    } else {
        vec![
            ("x-api-key", api_key.to_owned()),
            ("anthropic-version", ANTHROPIC_VERSION.to_owned()),
        ]
    };
    let headers =
        crate::request::merge_headers(crate::request::headers_for(model, base), extra.to_vec());
    let mut mapper = Mapper::new(model);
    let _ = sender.blocking_send(mapper.start_event());
    let retried = crate::request::waiting(sender);
    let resent = crate::request::pump_sse_with_resend(
        stop,
        || crate::request::send_with_retry(&url, &headers, body, proxy, &retried),
        |sse| {
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
        },
    )
    .map_err(|message| crate::request::auth_hint(&message, oauth, &model.provider))?;
    if let Some(first_error) = resent {
        crate::request::note_resend(&mut mapper.output, &first_error);
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
    let proxy = options.proxy.clone();
    let stop = options.stop.clone();
    let oauth = options.oauth;
    let extra_headers = options.extra_headers.clone();
    tokio::task::spawn_blocking(move || {
        let Err(message) = run_request(
            &model,
            &body,
            crate::request::Wire {
                api_key: &api_key,
                proxy: proxy.as_ref(),
                stop: stop.as_deref(),
                extra: &extra_headers,
                oauth,
            },
            &sender,
        ) else {
            return;
        };
        let mut mapper = Mapper::new(&model);
        let event = mapper.fail(&message);
        let _ = sender.blocking_send(event);
    });
    receiver
}
