use serde_json::{Value, json};
use tokio::sync::mpsc::{Receiver, Sender};
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{
    AgentMessage, Content, RAW_STOP_IN_BAND_ERROR, StopReason, Usage, UserContent,
};
use yi_types::model::{Effort, LlmContext, Model, ToolChoice, ToolDef};

use crate::breakpoints::{Breakpoints, CacheRoute, Dialect, Reuse, encode};
use crate::catalog::calculate_cost;
use crate::compat::{compat_bool, compat_str};
use crate::json_salvage::{parse_json_with_repair, parse_streaming_json};
use crate::transform::{system_text, transform_messages};

const REASONING_FIELDS: [&str; 3] = ["reasoning_content", "reasoning", "reasoning_text"];

#[derive(Debug, Clone, Default)]
pub struct OpenAiOptions {
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub reasoning_effort: Option<Effort>,
    pub session_id: Option<String>,
    pub proxy: Option<crate::request::ProxyConfig>,
    pub routing: Option<Value>,
    pub extra_headers: Vec<(String, String)>,
    /// A stored OAuth credential is on the wire (D191): a 401 names `yi login`.
    pub oauth: bool,
    /// The loop's cut of this request (D163): set, the pump stops at the next event.
    pub stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

/// A `sort` disables OpenRouter's price weighting and Auto Exacto and doubled row 0023's cost;
/// the deprioritisers push a slow upstream to the back of the list at catalog price.
pub const DEFAULT_ROUTING: &str =
    r#"{"preferred_min_throughput":{"p50":20},"preferred_max_latency":{"p50":10}}"#;

pub fn routing_params(model: &Model, options: &OpenAiOptions) -> Option<Value> {
    if !model.base_url.contains("openrouter.ai") {
        return None;
    }
    Some(
        options
            .routing
            .clone()
            .unwrap_or_else(|| serde_json::from_str(DEFAULT_ROUTING).unwrap_or(Value::Null)),
    )
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

/// Renders transformed messages onto `params`, recording for each rendered message the index
/// of the message it came from, which is what a breakpoint position names.
fn convert_messages(
    model: &Model,
    transformed: &[AgentMessage],
    params: &mut Vec<Value>,
    origins: &mut Vec<Option<usize>>,
) {
    let _span = yi_types::trace::span("ai.convert_messages");
    let mut index = 0;
    while index < transformed.len() {
        match &transformed[index] {
            AgentMessage::User { content, .. } => match content {
                UserContent::Text(text) => {
                    params.push(json!({"role": "user", "content": text}));
                    origins.push(Some(index));
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
                        origins.push(Some(index));
                    }
                }
            },
            AgentMessage::Assistant { content, .. } => {
                if let Some(message) = assistant_param(model, content) {
                    params.push(message);
                    origins.push(Some(index));
                }
            }
            AgentMessage::ToolResult { .. } => {
                index = push_tool_results(model, transformed, index, params, origins);
            }
            _ => {}
        }
        index = index.saturating_add(1);
    }
}

fn assistant_param(model: &Model, content: &[Content]) -> Option<Value> {
    {
        let text: String = content
            .iter()
            .filter_map(|block| match block {
                Content::Text { text, .. } if !text.trim().is_empty() => Some(text.as_str()),
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
        if compat_bool(model, "requiresReasoningContentOnAssistantMessages", false)
            && model.reasoning
            && message.get("reasoning_content").is_none()
        {
            message["reasoning_content"] = Value::String(String::new());
        }
        let has_content = message["content"].is_string();
        if has_content || message.get("tool_calls").is_some() {
            return Some(message);
        }
        None
    }
}

fn push_tool_results(
    model: &Model,
    transformed: &[AgentMessage],
    start: usize,
    params: &mut Vec<Value>,
    origins: &mut Vec<Option<usize>>,
) -> usize {
    let mut index = start;
    {
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
            origins.push(Some(index));
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
            let mut parts =
                vec![json!({"type": "text", "text": "Attached image(s) from tool result:"})];
            parts.extend(images);
            params.push(json!({"role": "user", "content": parts}));
            // Synthesized, so no position names it: the mark stays on the tool message's text.
            origins.push(None);
        }
    }
    index
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

fn convert_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Tool(forced) => {
            json!({"type": "function", "function": {"name": forced.as_str()}})
        }
    }
}

/// Extended retention is free on the models that take it; the id prefix and
/// the catalog flag keep the parameter off the ones where a 400 has no retry.
pub(crate) fn prompt_cache_retention(model: &Model) -> Option<&'static str> {
    (model.base_url.contains("api.openai.com")
        && model.id.starts_with("gpt-5.")
        && !compat_bool(model, "supportsExplicitPromptCacheMode", false))
    .then_some("24h")
}

pub fn build_params(model: &Model, context: &LlmContext, options: &OpenAiOptions) -> Value {
    let _span = yi_types::trace::span("ai.build_params")
        .arg("api", "openai")
        .arg("messages", context.messages.len());
    let history = transform_messages(
        &context.messages,
        model,
        Some(normalize_openai_tool_call_id),
    );
    let breakpoints =
        Breakpoints::build(&CacheRoute::of(model, false), &history, Reuse::of(context));
    let mut messages: Vec<Value> = Vec::new();
    let mut origins: Vec<Option<usize>> = Vec::new();
    if !context.system_prompt.is_empty() {
        let role = if model.reasoning && compat_bool(model, "supportsDeveloperRole", true) {
            "developer"
        } else {
            "system"
        };
        messages.push(json!({"role": role, "content": system_text(&context.system_prompt)}));
        origins.push(None);
    }
    convert_messages(model, &history, &mut messages, &mut origins);
    let mut params = json!({
        "model": model.id,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    // OpenRouter passes per-part marks to every upstream; elsewhere the provider places its own.
    let dialect = if model.base_url.contains("openrouter.ai") {
        Dialect::OpenRouterParts
    } else {
        Dialect::Automatic
    };
    encode(&breakpoints, dialect, &mut params, &origins);
    let transient = transform_messages(
        &context.transient,
        model,
        Some(normalize_openai_tool_call_id),
    );
    if let Some(list) = params.get_mut("messages").and_then(Value::as_array_mut) {
        convert_messages(model, &transient, list, &mut Vec::new());
    }
    if model.provider == "openai" {
        params["store"] = json!(false);
    }
    if model.base_url.contains("api.openai.com")
        && let Some(session_id) = &options.session_id
    {
        params["prompt_cache_key"] = json!(session_id);
    }
    if let Some(ttl) = prompt_cache_retention(model) {
        params["prompt_cache_retention"] = json!(ttl);
    }
    if let Some(routing) = routing_params(model, options) {
        params["provider"] = routing;
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
    if let Some(choice) = &context.tool_choice {
        params["tool_choice"] = convert_tool_choice(choice);
    }
    if model.reasoning {
        apply_reasoning_params(model, options, &mut params);
    }
    params
}

/// Invariant: callers clamp through [`Model::clamp_effort`] first, so the `null` arm is
/// unreachable; it stays as the safe answer rather than sending a rejected level.
pub(crate) fn mapped_effort(model: &Model, effort: Effort) -> Option<&str> {
    let Some(map) = model.thinking_level_map.as_ref() else {
        return Some(effort.as_str());
    };
    match map.get(effort.as_str()) {
        Some(Value::Null) => None,
        Some(value) => Some(value.as_str().unwrap_or(effort.as_str())),
        None => Some(effort.as_str()),
    }
}

fn apply_reasoning_params(model: &Model, options: &OpenAiOptions, params: &mut Value) {
    if compat_str(model, "thinkingFormat") == Some("openrouter") {
        let off = model
            .thinking_level_map
            .as_ref()
            .and_then(|map| map.get("off"));
        match options.reasoning_effort {
            Some(effort) => {
                if let Some(mapped) = mapped_effort(model, effort) {
                    params["reasoning"] = json!({"effort": mapped});
                }
            }
            None if !matches!(off, Some(Value::Null)) => {
                params["reasoning"] =
                    json!({"effort": off.and_then(Value::as_str).unwrap_or("none")});
            }
            None => {}
        }
    } else if let Some(effort) = options.reasoning_effort
        && let Some(mapped) = mapped_effort(model, effort)
    {
        params["reasoning_effort"] = json!(mapped);
    }
}

/// OpenRouter's mid-stream error rides at the chunk's top level beside the upstream's name.
fn error_chunk_text(chunk: &Value, error: &Value) -> String {
    let field = |key: &str| {
        error.get(key).map(|value| {
            value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned)
        })
    };
    let message = field("message").unwrap_or_else(|| "provider error".to_owned());
    let mut tail: Vec<String> = Vec::new();
    if let Some(upstream) = chunk.get("provider").and_then(Value::as_str) {
        tail.push(format!("upstream {upstream}"));
    }
    if let Some(code) = field("code") {
        tail.push(format!("code {code}"));
    }
    if let Some(kind) = error
        .get("metadata")
        .and_then(|m| m.get("error_type"))
        .and_then(Value::as_str)
    {
        tail.push(kind.to_owned());
    }
    if tail.is_empty() {
        message
    } else {
        format!("{message} ({})", tail.join(", "))
    }
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
    // OpenRouter reports the account charge; the catalog rates only approximate it.
    if model.provider == "openrouter"
        && let Some(cost) = raw.get("cost").and_then(Value::as_f64)
        && let Some(total) = serde_json::Number::from_f64(cost)
    {
        usage.cost.total = total;
    }
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
        if let Some(id) = chunk.get("id").and_then(Value::as_str)
            && let AgentMessage::Assistant { response_id, .. } = &mut self.output
            && response_id.is_none()
        {
            *response_id = Some(id.to_owned());
        }
        if let Some(upstream) = chunk.get("provider").and_then(Value::as_str) {
            crate::request::note_upstream(&mut self.output, upstream);
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
        if let Some(error) = chunk.get("error").filter(|error| error.is_object())
            && let AgentMessage::Assistant {
                stop_reason: output_stop,
                raw_stop_reason,
                error_message: output_error,
                ..
            } = &mut self.output
        {
            *output_stop = StopReason::Error;
            *raw_stop_reason = Some(RAW_STOP_IN_BAND_ERROR.to_owned());
            *output_error = Some(error_chunk_text(chunk, error));
            self.has_finish_reason = true;
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
                if error_message.is_some() && output_error.is_none() {
                    *output_error = error_message;
                }
            }
            self.has_finish_reason = true;
        }
        let Some(delta) = choice.get("delta") else {
            return events;
        };
        self.on_text_delta(delta, &mut events);
        self.on_thinking_delta(delta, &mut events);
        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for tool_call in tool_calls {
                self.on_tool_call_delta(tool_call, &mut events);
            }
        }
        events
    }

    fn on_text_delta(&mut self, delta: &Value, events: &mut Vec<AssistantMessageEvent>) {
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
                    events.push(AssistantMessageEvent::TextStart { content_index });
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
            });
        }
    }

    fn on_thinking_delta(&mut self, delta: &Value, events: &mut Vec<AssistantMessageEvent>) {
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
                    events.push(AssistantMessageEvent::ThinkingStart { content_index });
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
            });
            break;
        }
    }

    fn on_tool_call_delta(&mut self, tool_call: &Value, events: &mut Vec<AssistantMessageEvent>) {
        {
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
                        name: (!name.is_empty()).then(|| name.to_owned()),
                    });
                    self.tools.len().saturating_sub(1)
                });
            let (content_index, parsed, delta_text) = {
                let Some(slot) = self.tools.get_mut(slot_position) else {
                    return;
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
            });
        }
    }

    pub fn finish(mut self) -> Vec<AssistantMessageEvent> {
        let mut events = Vec::new();
        crate::leak::recover_in(&mut self.output);
        let content = self.content_mut().clone();
        for (content_index, block) in content.iter().enumerate() {
            match block {
                Content::Text { text, .. } => events.push(AssistantMessageEvent::TextEnd {
                    content_index,
                    content: text.clone(),
                }),
                Content::Thinking { thinking, .. } => {
                    events.push(AssistantMessageEvent::ThinkingEnd {
                        content_index,
                        content: thinking.clone(),
                    });
                }
                Content::ToolCall { .. } => events.push(AssistantMessageEvent::ToolCallEnd {
                    content_index,
                    tool_call: block.clone(),
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

    /// The stream was dropped at the reasoning budget (D163): no finish reason will come, and
    /// a cut is a length stop by definition, so `finish` ends it as a `Done`, not an error.
    pub fn cut(&mut self) {
        self.has_finish_reason = true;
        if let AgentMessage::Assistant { stop_reason, .. } = &mut self.output {
            *stop_reason = StopReason::Length;
        }
    }
}

/// A turn whose stream never reached its usage chunk, settled from the generation record
/// when a chunk carried the id and the usage is still unknown (D163).
fn settle_from_record(
    output: &mut AgentMessage,
    model: &Model,
    api_key: &str,
    proxy: Option<&crate::request::ProxyConfig>,
) {
    if let AgentMessage::Assistant {
        usage,
        response_id: Some(id),
        ..
    } = output
        && usage.unknown
        && let Some((settled, upstream)) =
            crate::settle::generation_usage(model, api_key, proxy, id)
    {
        *usage = settled;
        if let Some(upstream) = upstream {
            crate::request::note_upstream(output, &upstream);
        }
    }
}

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
    let url = format!("{}/chat/completions", model.base_url);
    let mut mapper = ChunkMapper::new(model);
    let _ = sender.blocking_send(mapper.start_event());
    let retried = crate::request::waiting(sender);
    let pumped = crate::request::pump_sse_with_resend(
        stop,
        || crate::request::openai_bearer_post(&url, model, api_key, body, proxy, extra, &retried),
        |sse| {
            if sse.data == "[DONE]" {
                return Ok(true);
            }
            if let Ok(payload) = parse_json_with_repair(&sse.data) {
                for event in mapper.push_chunk(&payload) {
                    let _ = sender.blocking_send(event);
                }
            }
            Ok(true)
        },
    );
    let resent = match pumped {
        Ok(resent) => resent,
        // a stream that died after its first chunk was billed for what it generated: the
        // error turn is settled from the record (D79, #331); one that died before any chunk
        // has no record and stays unknown
        Err(message) => {
            settle_from_record(&mut mapper.output, model, api_key, proxy);
            let _ = sender.blocking_send(mapper.fail(&crate::request::auth_hint(
                &message,
                oauth,
                &model.provider,
            )));
            return Ok(());
        }
    };
    if let Some(first_error) = resent {
        crate::request::note_resend(&mut mapper.output, &first_error);
    }
    // a cut dropped the stream before its finish and usage chunks: the turn ends as a length
    // stop and is settled from the generation record (D163)
    if stop.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst)) {
        mapper.cut();
        settle_from_record(&mut mapper.output, model, api_key, proxy);
    }
    // an in-band error chunk ends the stream before its usage chunk: settled the same way (D175)
    if matches!(
        mapper.output,
        AgentMessage::Assistant {
            stop_reason: StopReason::Error,
            ..
        }
    ) {
        settle_from_record(&mut mapper.output, model, api_key, proxy);
    }
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
    crate::request::spawn_stream(
        |model, message| ChunkMapper::new(model).fail(message),
        model,
        build_params(model, context, options),
        crate::request::WireOwned {
            api_key: api_key.to_owned(),
            proxy: options.proxy.clone(),
            stop: options.stop.clone(),
            extra: options.extra_headers.clone(),
            oauth: options.oauth,
        },
        run_request,
    )
}
