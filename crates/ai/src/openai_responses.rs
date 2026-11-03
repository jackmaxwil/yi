use serde_json::{Map, Value, json};
use tokio::sync::mpsc::{Receiver, Sender};
use yi_types::event::AssistantMessageEvent;
use yi_types::message::{AgentMessage, Content, StopReason};
use yi_types::model::{LlmContext, Model};

mod encode;

use crate::breakpoints::Encoded;
use crate::catalog::calculate_cost;
use crate::json_salvage::{parse_json_with_repair, parse_streaming_json};
use crate::openai::OpenAiOptions;
use crate::request::{assistant_content_mut, content_end_events};
pub use encode::{build_params, normalize_responses_tool_call_id};
use encode::{combined_id, freeform_arguments};

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
    refusal: String,
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
            refusal: String::new(),
        }
    }

    pub fn start_event(&self) -> AssistantMessageEvent {
        AssistantMessageEvent::Start {
            partial: self.output.clone(),
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
        let content = assistant_content_mut(&mut self.output)?;
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
        let content = assistant_content_mut(&mut self.output)?;
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
                    assistant_content_mut(&mut self.output)
                        .and_then(|content| content.get_mut(content_index))
                })
                && slot_name.is_empty()
            {
                *slot_name = name.to_owned();
            }
            return Some(index);
        }
        self.text_index = None;
        let content = assistant_content_mut(&mut self.output)?;
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
        if let Some(Content::Text { text, .. }) = assistant_content_mut(&mut self.output)
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
        if let Some(Content::Thinking { thinking, .. }) = assistant_content_mut(&mut self.output)
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
        if let Some(Content::ToolCall { id, arguments, .. }) =
            assistant_content_mut(&mut self.output)
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
        events.push(AssistantMessageEvent::ToolCallDelta {
            content_index: slot.content_index,
            delta: chunk.to_owned(),
        });
    }

    fn settle_open_calls(&mut self) {
        for slot_index in 0..self.tools.len() {
            if self.tools.get(slot_index).is_some_and(|slot| !slot.ended) {
                let _ = self.materialize_args(slot_index);
            }
        }
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
            if let Some(tool_call) = assistant_content_mut(&mut self.output)
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
            Some("reasoning") => self.thinking_index = None,
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
                    && let Some(Content::Text { text_signature, .. }) =
                        assistant_content_mut(&mut self.output)
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
        }) = assistant_content_mut(&mut self.output)
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
        let has_tools = assistant_content_mut(&mut self.output)
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
        if self.refusal.is_empty() {
            self.set_stop(reason, None);
        } else {
            // Incident: `response.refusal.*` ended the turn as an empty Stop and the loop read it
            // as an answer with no text; the refusal is the provider's answer, named.
            self.set_stop(
                StopReason::Error,
                Some(format!("The model refused: {}", self.refusal)),
            );
        }
        self.completed = true;
    }

    fn on_failed(&mut self, payload: &Value) -> AssistantMessageEvent {
        let message = payload
            .pointer("/error/message")
            .or_else(|| payload.pointer("/response/error/message"))
            .or_else(|| payload.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("response.failed");
        self.settle_open_calls();
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
            "response.refusal.delta" => {
                if let Some(delta) = payload.get("delta").and_then(Value::as_str) {
                    self.refusal.push_str(delta);
                }
            }
            "response.refusal.done" => {
                if let Some(refusal) = payload.get("refusal").and_then(Value::as_str) {
                    self.refusal = refusal.to_owned();
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
        self.settle_open_calls();
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
        let mut events = content_end_events(&mut self.output, |content_index| {
            !self
                .tools
                .iter()
                .any(|slot| slot.content_index == content_index && slot.ended)
        });
        events.push(crate::request::terminal_event(self.output.clone()));
        events
    }

    pub fn fail(&mut self, message: &str) -> AssistantMessageEvent {
        self.settle_open_calls();
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
    let retry = crate::request::Retry::new(model.read_timeout(), wire.stop, &retried);
    let resent = match crate::request::pump_sse_with_resend(
        wire.stop,
        &retry,
        || crate::request::openai_bearer_post(&url, model, body, wire, &retry),
        |sse| {
            if sse.data == "[DONE]" {
                return Ok(true);
            }
            let data = sse.data.trim();
            if data.is_empty() {
                return Ok(true);
            }
            let mut payload = parse_json_with_repair(data)
                .map_err(|error| format!("Could not parse Responses SSE event: {error}"))?;
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
    ) {
        Ok(resent) => resent,
        // Incident: an error after the first delta threw the turn away, because the fresh mapper in
        // `stream` was failed instead of this one; the text and usage were billed.
        Err(message) => {
            let _ = sender.blocking_send(mapper.fail(&crate::request::auth_hint(
                &message,
                wire.oauth,
                &model.provider,
            )));
            return Ok(());
        }
    };
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
            settle: None,
        },
        run_request,
    )
}
