use serde_json::{Map, Value, json};
use tokio::sync::mpsc::{Receiver, Sender};
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
use yi_types::model::{LlmContext, Model, ToolChoice, ToolDef};

use crate::breakpoints::Encoded;
use crate::catalog::calculate_cost;
use crate::compat::developer_role;
use crate::json_salvage::{parse_json_with_repair, parse_streaming_json};
use crate::openai::{OpenAiOptions, mapped_effort};
use crate::transform::{system_text, transform_messages};

pub fn normalize_responses_tool_call_id(id: &str) -> String {
    id.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric()
                || character == '_'
                || character == '-'
                || character == '|'
            {
                character
            } else {
                '_'
            }
        })
        .take(128)
        .collect()
}

fn call_and_item(id: &str) -> (&str, Option<&str>) {
    match id.split_once('|') {
        Some((call, item)) if !item.is_empty() => (call, Some(item)),
        _ => (id, None),
    }
}

fn combined_id(call_id: &str, item_id: &str) -> String {
    if item_id.is_empty() {
        call_id.to_owned()
    } else {
        format!("{call_id}|{item_id}")
    }
}

fn convert_user(content: &UserContent) -> Option<Value> {
    match content {
        UserContent::Text(text) => Some(json!({
            "role": "user",
            "content": [{"type": "input_text", "text": text}],
        })),
        UserContent::Blocks(blocks) => {
            let parts: Vec<Value> = blocks
                .iter()
                .filter_map(|block| match block {
                    Content::Text { text, .. } => Some(json!({"type": "input_text", "text": text})),
                    Content::Image { data, mime_type } => Some(json!({
                        "type": "input_image",
                        "detail": "auto",
                        "image_url": format!("data:{mime_type};base64,{data}"),
                    })),
                    _ => None,
                })
                .collect();
            (!parts.is_empty()).then(|| json!({"role": "user", "content": parts}))
        }
    }
}

/// The one raw argument a freeform (custom) tool call carries. C8 scopes freeform to the
/// hashline edit tool; thread a per-tool key when a second freeform tool exists.
const FREEFORM_ARGUMENT: &str = "patch";

fn convert_assistant(
    content: &[Content],
    freeform: &std::collections::BTreeSet<String>,
) -> Vec<Value> {
    let mut items = Vec::new();
    for block in content {
        match block {
            Content::Thinking {
                thinking_signature, ..
            } => {
                if let Some(signature) = thinking_signature
                    && let Ok(item) = serde_json::from_str::<Value>(signature)
                    && item.get("type").and_then(Value::as_str) == Some("reasoning")
                {
                    items.push(item);
                }
            }
            Content::Text {
                text,
                text_signature,
            } if !text.is_empty() => {
                let mut item = json!({
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": text, "annotations": []}],
                });
                if let Some(id) = text_signature.as_ref().filter(|value| !value.is_empty()) {
                    item["id"] = json!(id);
                }
                items.push(item);
            }
            Content::ToolCall {
                id,
                name,
                arguments,
                ..
            } => {
                let (call_id, item_id) = call_and_item(id);
                let mut item = if freeform.contains(name) {
                    let input = arguments
                        .get(FREEFORM_ARGUMENT)
                        .and_then(Value::as_str)
                        .map_or_else(
                            || Value::Object(arguments.clone()).to_string(),
                            str::to_owned,
                        );
                    json!({
                        "type": "custom_tool_call",
                        "call_id": call_id,
                        "name": name,
                        "input": input,
                    })
                } else {
                    json!({
                        "type": "function_call",
                        "call_id": call_id,
                        "name": name,
                        "arguments": Value::Object(arguments.clone()).to_string(),
                    })
                };
                if let Some(item_id) = item_id {
                    item["id"] = json!(item_id);
                }
                items.push(item);
            }
            _ => {}
        }
    }
    items
}

fn convert_tool_result(
    tool_call_id: &str,
    content: &[Content],
    vision: bool,
    is_freeform: bool,
) -> Value {
    let kind = if is_freeform {
        "custom_tool_call_output"
    } else {
        "function_call_output"
    };
    let (call_id, _) = call_and_item(tool_call_id);
    let text: String = content
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let images: Vec<Value> = if vision {
        content
            .iter()
            .filter_map(|block| match block {
                Content::Image { data, mime_type } => Some(json!({
                    "type": "input_image",
                    "detail": "auto",
                    "image_url": format!("data:{mime_type};base64,{data}"),
                })),
                _ => None,
            })
            .collect()
    } else {
        Vec::new()
    };
    if images.is_empty() {
        let output = if text.is_empty() {
            "(no tool output)"
        } else {
            text.as_str()
        };
        json!({
            "type": kind,
            "call_id": call_id,
            "output": output,
        })
    } else {
        let mut parts = Vec::new();
        if !text.is_empty() {
            parts.push(json!({"type": "input_text", "text": text}));
        }
        parts.extend(images);
        json!({
            "type": kind,
            "call_id": call_id,
            "output": parts,
        })
    }
}

fn freeform_names(context: &LlmContext) -> std::collections::BTreeSet<String> {
    context
        .tools
        .iter()
        .flatten()
        .filter(|tool| tool.freeform.is_some())
        .map(|tool| tool.name.clone())
        .collect()
}

/// The system and history items, then the per-request items that render after them.
fn convert_input(model: &Model, context: &LlmContext) -> (Vec<Value>, Vec<Value>) {
    let _span = yi_types::trace::span("ai.convert_messages");
    let mut input = Vec::new();
    // The `openai-codex` backend takes the system text as top-level `instructions`;
    // sending it as an input message too would bill it twice.
    let codex = model.provider == "openai-codex";
    if !context.system_prompt.is_empty() && !codex {
        let role = if developer_role(model) {
            "developer"
        } else {
            "system"
        };
        input.push(json!({"role": role, "content": system_text(&context.system_prompt)}));
    }
    let vision = model.input.iter().any(|kind| kind == "image");
    let freeform = freeform_names(context);
    let history = transform_messages(
        &context.messages,
        model,
        Some(normalize_responses_tool_call_id),
    );
    let transient = transform_messages(
        &context.transient,
        model,
        Some(normalize_responses_tool_call_id),
    );
    let items = |messages: &[AgentMessage]| {
        let mut out = Vec::new();
        for message in messages {
            match message {
                AgentMessage::User { content, .. } => {
                    if let Some(item) = convert_user(content) {
                        out.push(item);
                    }
                }
                AgentMessage::Assistant { content, .. } => {
                    out.extend(convert_assistant(content, &freeform));
                }
                AgentMessage::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    ..
                } => out.push(convert_tool_result(
                    tool_call_id,
                    content,
                    vision,
                    freeform.contains(tool_name),
                )),
                _ => {}
            }
        }
        out
    };
    input.extend(items(&history));
    (input, items(&transient))
}

/// A custom tool call's raw text becomes the one argument the tool schema
/// names, so the tool layer never learns which wire shape delivered it.
fn freeform_arguments(input: &str) -> Map<String, Value> {
    let mut arguments = Map::new();
    arguments.insert(
        FREEFORM_ARGUMENT.to_owned(),
        Value::String(input.to_owned()),
    );
    arguments
}

fn convert_tools(tools: &[ToolDef]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| match &tool.freeform {
            Some(format) => json!({
                "type": "custom",
                "name": tool.name,
                "description": tool.description,
                "format": {
                    "type": "grammar",
                    "syntax": "lark",
                    "definition": format.definition,
                },
            }),
            None => json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
                "strict": false,
            }),
        })
        .collect()
}

fn convert_tool_choice(choice: &ToolChoice, tools: Option<&[ToolDef]>) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Tool(forced) => {
            let emitted_as_custom = tools.is_some_and(|tools| {
                tools
                    .iter()
                    .any(|tool| tool.name == forced.as_str() && tool.freeform.is_some())
            });
            let kind = if emitted_as_custom {
                "custom"
            } else {
                "function"
            };
            json!({"type": kind, "name": forced.as_str()})
        }
    }
}

fn apply_reasoning(model: &Model, options: &OpenAiOptions, params: &mut Value) {
    let off = model
        .thinking_level_map
        .as_ref()
        .and_then(|map| map.get("off"));
    match options.reasoning_effort {
        Some(effort) => {
            if let Some(mapped) = mapped_effort(model, effort) {
                params["reasoning"] = json!({"effort": mapped});
                params["include"] = json!(["reasoning.encrypted_content"]);
            }
        }
        None if model.reasoning && !matches!(off, Some(Value::Null)) => {
            params["reasoning"] = json!({
                "effort": off.and_then(Value::as_str).unwrap_or("none")
            });
        }
        None => {}
    }
}

pub fn build_params(model: &Model, context: &LlmContext, options: &OpenAiOptions) -> Encoded {
    let _span = yi_types::trace::span("ai.build_params")
        .arg("api", "responses")
        .arg("messages", context.messages.len());
    let (input, transient) = convert_input(model, context);
    let mut params = json!({
        "model": model.id,
        "input": input,
        "stream": true,
        "store": false,
    });
    if model.provider == "openai-codex" {
        params["instructions"] = json!(system_text(&context.system_prompt));
    }
    if let Some(session_id) = &options.session_id {
        params["prompt_cache_key"] = json!(session_id);
    }
    if let Some(ttl) = crate::openai::prompt_cache_retention(model) {
        params["prompt_cache_retention"] = json!(ttl);
    }
    if let Some(schema) = &context.schema {
        params["text"]["format"] = crate::schema::responses(schema);
    }
    if let Some(max_tokens) = options.max_tokens {
        params["max_output_tokens"] = json!(max_tokens);
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
        params["tool_choice"] = convert_tool_choice(choice, context.tools.as_deref());
    }
    apply_reasoning(model, options, &mut params);
    // No explicit breakpoint is known to be accepted on this wire; the provider caches its own
    // prefix, and the per-request facts still render last (design §11).
    Encoded::provider_prefix(params, "input", transient)
}

fn parse_usage(raw: &Value, model: &Model) -> yi_types::message::Usage {
    let get = |value: &Value, key: &str| value.get(key).and_then(Value::as_i64).unwrap_or(0);
    let prompt = get(raw, "input_tokens");
    let details = raw.get("input_tokens_details");
    let detail = |key: &str| {
        details
            .and_then(|value| value.get(key))
            .and_then(Value::as_i64)
            .unwrap_or(0)
    };
    let cache_read = detail("cached_tokens");
    let cache_write = detail("cache_write_tokens");
    let output = get(raw, "output_tokens");
    let mut usage = yi_types::message::Usage::zero();
    usage.input = prompt
        .saturating_sub(cache_read)
        .saturating_sub(cache_write);
    usage.output = output;
    usage.cache_read = cache_read;
    usage.cache_write = cache_write;
    usage.reasoning = raw
        .get("output_tokens_details")
        .and_then(|details| details.get("reasoning_tokens"))
        .and_then(Value::as_i64);
    usage.total_tokens = usage
        .input
        .saturating_add(output)
        .saturating_add(cache_read)
        .saturating_add(cache_write);
    calculate_cost(model, &mut usage);
    usage
}

struct ToolSlot {
    content_index: usize,
    item_id: String,
    call_id: String,
    ended: bool,
    partial_args: String,
    freeform: bool,
}

pub struct EventMapper {
    output: AgentMessage,
    model: Model,
    text_index: Option<usize>,
    thinking_index: Option<usize>,
    tools: Vec<ToolSlot>,
    completed: bool,
    failed: bool,
    closed: bool,
}

impl EventMapper {
    pub fn new(model: &Model) -> Self {
        Self {
            output: crate::request::empty_assistant(model),
            model: model.clone(),
            text_index: None,
            thinking_index: None,
            tools: Vec::new(),
            completed: false,
            failed: false,
            closed: false,
        }
    }

    pub fn start_event(&self) -> AssistantMessageEvent {
        AssistantMessageEvent::Start {
            partial: self.output.clone(),
        }
    }

    fn content_mut(&mut self) -> Option<&mut Vec<Content>> {
        match &mut self.output {
            AgentMessage::Assistant { content, .. } => Some(content),
            _ => None,
        }
    }

    fn set_response_id(&mut self, id: &str) {
        if let AgentMessage::Assistant {
            response_id: slot, ..
        } = &mut self.output
            && slot.is_none()
        {
            *slot = Some(id.to_owned());
        }
    }

    fn apply_usage(&mut self, usage: &Value) {
        if let AgentMessage::Assistant {
            usage: output_usage,
            ..
        } = &mut self.output
        {
            *output_usage = parse_usage(usage, &self.model);
        }
    }

    fn set_stop(&mut self, reason: StopReason, error: Option<String>) {
        if let AgentMessage::Assistant {
            stop_reason,
            error_message,
            ..
        } = &mut self.output
        {
            *stop_reason = reason;
            if error.is_some() {
                *error_message = error;
            }
        }
    }

    fn ensure_text(&mut self, events: &mut Vec<AssistantMessageEvent>) -> Option<usize> {
        if let Some(existing) = self.text_index {
            return Some(existing);
        }
        let content = self.content_mut()?;
        let content_index = content.len();
        content.push(Content::Text {
            text: String::new(),
            text_signature: None,
        });
        self.text_index = Some(content_index);
        events.push(AssistantMessageEvent::TextStart { content_index });
        Some(content_index)
    }

    fn ensure_thinking(&mut self, events: &mut Vec<AssistantMessageEvent>) -> Option<usize> {
        if let Some(existing) = self.thinking_index {
            return Some(existing);
        }
        let content = self.content_mut()?;
        let content_index = content.len();
        content.push(Content::Thinking {
            thinking: String::new(),
            thinking_signature: None,
            redacted: None,
        });
        self.thinking_index = Some(content_index);
        events.push(AssistantMessageEvent::ThinkingStart { content_index });
        Some(content_index)
    }

    fn mark_freeform(&mut self, slot_index: usize) {
        if let Some(slot) = self.tools.get_mut(slot_index) {
            slot.freeform = true;
        }
    }

    /// One handler for a `custom_tool_call` item, streamed open or done.
    fn on_custom_tool_call(
        &mut self,
        item: &Value,
        done: bool,
        events: &mut Vec<AssistantMessageEvent>,
    ) {
        let item_id = item.get("id").and_then(Value::as_str).unwrap_or("");
        let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
        let name = item.get("name").and_then(Value::as_str).unwrap_or("");
        let Some(slot) = self.ensure_tool(item_id, call_id, name, events) else {
            return;
        };
        self.mark_freeform(slot);
        let input = item.get("input").and_then(Value::as_str);
        if done {
            self.end_tool(slot, input, events);
        } else if let Some(input) = input.filter(|input| !input.is_empty()) {
            self.apply_tool_args(slot, input, events);
        }
    }

    fn find_tool(&mut self, item_id: &str, call_id: &str) -> Option<usize> {
        if item_id.is_empty() && call_id.is_empty() {
            return self.tools.iter().rposition(|slot| !slot.ended);
        }
        self.tools.iter().position(|slot| {
            (!item_id.is_empty() && slot.item_id == item_id)
                || (!call_id.is_empty() && slot.call_id == call_id)
        })
    }

    fn ensure_tool(
        &mut self,
        item_id: &str,
        call_id: &str,
        name: &str,
        events: &mut Vec<AssistantMessageEvent>,
    ) -> Option<usize> {
        if let Some(index) = self.find_tool(item_id, call_id) {
            let mut backfill_index = None;
            if let Some(slot) = self.tools.get_mut(index) {
                if slot.item_id.is_empty() && !item_id.is_empty() {
                    slot.item_id = item_id.to_owned();
                }
                if slot.call_id.is_empty() && !call_id.is_empty() {
                    slot.call_id = call_id.to_owned();
                }
                backfill_index = Some(slot.content_index);
            }
            if !name.is_empty()
                && let Some(Content::ToolCall {
                    name: slot_name, ..
                }) = backfill_index.and_then(|content_index| {
                    self.content_mut()
                        .and_then(|content| content.get_mut(content_index))
                })
                && slot_name.is_empty()
            {
                *slot_name = name.to_owned();
            }
            return Some(index);
        }
        let content = self.content_mut()?;
        let content_index = content.len();
        content.push(Content::ToolCall {
            id: combined_id(call_id, item_id),
            name: name.to_owned(),
            arguments: Map::new(),
            thought_signature: None,
            namespace: None,
        });
        self.tools.push(ToolSlot {
            content_index,
            item_id: item_id.to_owned(),
            call_id: call_id.to_owned(),
            ended: false,
            partial_args: String::new(),
            freeform: false,
        });
        events.push(AssistantMessageEvent::ToolCallStart {
            content_index,
            name: (!name.is_empty()).then(|| name.to_owned()),
        });
        Some(self.tools.len().saturating_sub(1))
    }

    fn append_text(&mut self, delta: &str, events: &mut Vec<AssistantMessageEvent>) {
        if delta.is_empty() {
            return;
        }
        let Some(content_index) = self.ensure_text(events) else {
            return;
        };
        if let Some(Content::Text { text, .. }) = self
            .content_mut()
            .and_then(|content| content.get_mut(content_index))
        {
            text.push_str(delta);
        }
        events.push(AssistantMessageEvent::TextDelta {
            content_index,
            delta: delta.to_owned(),
        });
    }

    fn append_thinking(&mut self, delta: &str, events: &mut Vec<AssistantMessageEvent>) {
        if delta.is_empty() {
            return;
        }
        let Some(content_index) = self.ensure_thinking(events) else {
            return;
        };
        if let Some(Content::Thinking { thinking, .. }) = self
            .content_mut()
            .and_then(|content| content.get_mut(content_index))
        {
            thinking.push_str(delta);
        }
        events.push(AssistantMessageEvent::ThinkingDelta {
            content_index,
            delta: delta.to_owned(),
        });
    }

    /// Parse the slot's accumulated arguments (raw text for a freeform slot)
    /// into its tool-call content block; returns the block's index.
    fn materialize_args(&mut self, slot_index: usize) -> Option<usize> {
        let slot = self.tools.get(slot_index)?;
        let parsed = if slot.freeform {
            freeform_arguments(&slot.partial_args)
        } else {
            parse_streaming_json(&slot.partial_args)
        };
        let content_index = slot.content_index;
        let call_id = slot.call_id.clone();
        let item_id = slot.item_id.clone();
        if let Some(Content::ToolCall { id, arguments, .. }) = self
            .content_mut()
            .and_then(|content| content.get_mut(content_index))
        {
            *id = combined_id(&call_id, &item_id);
            *arguments = parsed;
        }
        Some(content_index)
    }

    fn apply_tool_args(
        &mut self,
        slot_index: usize,
        chunk: &str,
        events: &mut Vec<AssistantMessageEvent>,
    ) {
        let Some(slot) = self.tools.get_mut(slot_index) else {
            return;
        };
        if slot.ended {
            return;
        }
        slot.partial_args.push_str(chunk);
        let Some(content_index) = self.materialize_args(slot_index) else {
            return;
        };
        events.push(AssistantMessageEvent::ToolCallDelta {
            content_index,
            delta: chunk.to_owned(),
        });
    }

    fn end_tool(
        &mut self,
        slot_index: usize,
        arguments: Option<&str>,
        events: &mut Vec<AssistantMessageEvent>,
    ) {
        if let Some(slot) = self.tools.get_mut(slot_index) {
            if slot.ended {
                return;
            }
            if let Some(arguments) = arguments {
                slot.partial_args = arguments.to_owned();
            }
            slot.ended = true;
            let Some(content_index) = self.materialize_args(slot_index) else {
                return;
            };
            if let Some(tool_call) = self
                .content_mut()
                .and_then(|content| content.get(content_index))
                .cloned()
            {
                events.push(AssistantMessageEvent::ToolCallEnd {
                    content_index,
                    tool_call,
                });
            }
        }
    }

    fn on_item_added(&mut self, item: &Value, events: &mut Vec<AssistantMessageEvent>) {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                let item_id = item.get("id").and_then(Value::as_str).unwrap_or("");
                let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
                let name = item.get("name").and_then(Value::as_str).unwrap_or("");
                let _ = self.ensure_tool(item_id, call_id, name, events);
            }
            Some("custom_tool_call") => self.on_custom_tool_call(item, false, events),
            Some("message") => {
                if let Some(id) = item.get("id").and_then(Value::as_str)
                    && let Some(content_index) = self.ensure_text(events)
                    && let Some(Content::Text { text_signature, .. }) = self
                        .content_mut()
                        .and_then(|content| content.get_mut(content_index))
                {
                    *text_signature = Some(id.to_owned());
                }
            }
            _ => {}
        }
    }

    fn on_item_done(&mut self, item: &Value, events: &mut Vec<AssistantMessageEvent>) {
        if item.get("type").and_then(Value::as_str) == Some("custom_tool_call") {
            self.on_custom_tool_call(item, true, events);
            return;
        }
        if item.get("type").and_then(Value::as_str) != Some("reasoning") {
            return;
        }
        let Some(content_index) = self.ensure_thinking(events) else {
            return;
        };
        if let Some(Content::Thinking {
            thinking_signature, ..
        }) = self
            .content_mut()
            .and_then(|content| content.get_mut(content_index))
        {
            *thinking_signature = Some(item.to_string());
        }
    }

    fn on_completed(&mut self, response: &Value, force_length: bool) {
        if let Some(id) = response.get("id").and_then(Value::as_str) {
            self.set_response_id(id);
        }
        if let Some(usage) = response.get("usage") {
            self.apply_usage(usage);
        }
        let has_tools = self
            .content_mut()
            .map(|content| {
                content
                    .iter()
                    .any(|block| matches!(block, Content::ToolCall { .. }))
            })
            .unwrap_or(false);
        let reason = if force_length {
            StopReason::Length
        } else if has_tools {
            StopReason::ToolUse
        } else {
            StopReason::Stop
        };
        self.set_stop(reason, None);
        self.completed = true;
    }

    fn on_failed(&mut self, payload: &Value) -> AssistantMessageEvent {
        let message = payload
            .pointer("/error/message")
            .or_else(|| payload.pointer("/response/error/message"))
            .or_else(|| payload.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("response.failed");
        self.failed = true;
        crate::request::fail_message(&mut self.output, message)
    }

    pub fn push(&mut self, payload: &Value) -> Vec<AssistantMessageEvent> {
        let kind = payload.get("type").and_then(Value::as_str).unwrap_or("");
        let mut events = Vec::new();
        match kind {
            "response.created" => {
                if let Some(id) = payload
                    .pointer("/response/id")
                    .or_else(|| payload.get("id"))
                    .and_then(Value::as_str)
                {
                    self.set_response_id(id);
                }
            }
            "response.output_text.delta" => {
                if let Some(delta) = payload.get("delta").and_then(Value::as_str) {
                    self.append_text(delta, &mut events);
                }
            }
            "response.reasoning_text.delta" | "response.reasoning_summary_text.delta" => {
                if let Some(delta) = payload.get("delta").and_then(Value::as_str) {
                    self.append_thinking(delta, &mut events);
                }
            }
            "response.output_item.added" => {
                if let Some(item) = payload.get("item") {
                    self.on_item_added(item, &mut events);
                }
            }
            "response.output_item.done" => {
                if let Some(item) = payload.get("item") {
                    self.on_item_done(item, &mut events);
                }
            }
            "response.custom_tool_call_input.delta" => {
                let item_id = payload.get("item_id").and_then(Value::as_str).unwrap_or("");
                let call_id = payload.get("call_id").and_then(Value::as_str).unwrap_or("");
                let Some(slot) = self.ensure_tool(item_id, call_id, "", &mut events) else {
                    return events;
                };
                self.mark_freeform(slot);
                if let Some(delta) = payload.get("delta").and_then(Value::as_str) {
                    self.apply_tool_args(slot, delta, &mut events);
                }
            }
            "response.custom_tool_call_input.done" => {
                let item_id = payload.get("item_id").and_then(Value::as_str).unwrap_or("");
                let call_id = payload.get("call_id").and_then(Value::as_str).unwrap_or("");
                let Some(slot) = self.ensure_tool(item_id, call_id, "", &mut events) else {
                    return events;
                };
                self.mark_freeform(slot);
                self.end_tool(
                    slot,
                    payload.get("input").and_then(Value::as_str),
                    &mut events,
                );
            }
            "response.function_call_arguments.delta" => {
                let item_id = payload.get("item_id").and_then(Value::as_str).unwrap_or("");
                let Some(slot) = self.ensure_tool(item_id, "", "", &mut events) else {
                    return events;
                };
                if let Some(delta) = payload.get("delta").and_then(Value::as_str) {
                    self.apply_tool_args(slot, delta, &mut events);
                }
            }
            "response.function_call_arguments.done" => {
                let item_id = payload.get("item_id").and_then(Value::as_str).unwrap_or("");
                let call_id = payload.get("call_id").and_then(Value::as_str).unwrap_or("");
                let name = payload.get("name").and_then(Value::as_str).unwrap_or("");
                let Some(slot) = self.ensure_tool(item_id, call_id, name, &mut events) else {
                    return events;
                };
                self.end_tool(
                    slot,
                    payload.get("arguments").and_then(Value::as_str),
                    &mut events,
                );
            }
            "response.completed" => {
                if let Some(response) = payload.get("response") {
                    self.on_completed(response, false);
                }
            }
            "response.incomplete" => {
                if let Some(response) = payload.get("response") {
                    self.on_completed(response, true);
                }
            }
            "response.failed" | "error" => {
                events.push(self.on_failed(payload));
            }
            _ => {}
        }
        events
    }

    pub fn finish(&mut self) -> Vec<AssistantMessageEvent> {
        if self.closed {
            return Vec::new();
        }
        self.closed = true;
        if self.failed {
            return Vec::new();
        }
        let mut events = Vec::new();
        if !self.completed {
            events.push(crate::request::fail_message(
                &mut self.output,
                "Stream ended without response.completed",
            ));
            return events;
        }
        crate::leak::recover_in(&mut self.output);
        let content = self.content_mut().cloned().unwrap_or_default();
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
                Content::ToolCall { .. } => {
                    let already = self
                        .tools
                        .iter()
                        .any(|slot| slot.content_index == content_index && slot.ended);
                    if !already {
                        events.push(AssistantMessageEvent::ToolCallEnd {
                            content_index,
                            tool_call: block.clone(),
                        });
                    }
                }
                Content::Image { .. } => {}
            }
        }
        events.push(crate::request::terminal_event(self.output.clone()));
        events
    }

    pub fn fail(&mut self, message: &str) -> AssistantMessageEvent {
        self.failed = true;
        crate::request::fail_message(&mut self.output, message)
    }
}

fn run_request(
    model: &Model,
    body: &Encoded,
    wire: crate::request::Wire<'_>,
    sender: &Sender<AssistantMessageEvent>,
) -> Result<(), String> {
    let url = format!("{}/responses", model.base_url);
    let mut mapper = EventMapper::new(model);
    let _ = sender.blocking_send(mapper.start_event());
    let retried = crate::request::waiting(sender);
    let resent = crate::request::pump_sse_with_resend(
        wire.stop,
        || {
            crate::request::openai_bearer_post(
                &url,
                model,
                wire.api_key,
                body,
                wire.proxy,
                wire.extra,
                &retried,
            )
        },
        |sse| {
            if sse.data == "[DONE]" {
                return Ok(true);
            }
            let Ok(mut payload) = parse_json_with_repair(&sse.data) else {
                return Ok(true);
            };
            if payload.get("type").is_none()
                && let Some(kind) = sse.event.as_deref().filter(|kind| !kind.is_empty())
            {
                payload["type"] = json!(kind);
            }
            for item in mapper.push(&payload) {
                let _ = sender.blocking_send(item);
            }
            if mapper.failed || mapper.completed {
                for item in mapper.finish() {
                    let _ = sender.blocking_send(item);
                }
                return Ok(false);
            }
            Ok(true)
        },
    )
    .map_err(|message| crate::request::auth_hint(&message, wire.oauth, &model.provider))?;
    if let Some(first_error) = resent {
        crate::request::note_resend(&mut mapper.output, &first_error);
    }
    for item in mapper.finish() {
        let _ = sender.blocking_send(item);
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
        |model, message| EventMapper::new(model).fail(message),
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
