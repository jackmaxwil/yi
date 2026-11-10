use serde_json::{Map, Value, json};
use std::borrow::Cow;
use yi_types::message::{AgentMessage, Content, UserContent};
use yi_types::model::{LlmContext, Model, ToolChoice, ToolDef};

use crate::breakpoints::Encoded;
use crate::compat::developer_role;
use crate::openai::{OpenAiOptions, mapped_effort};
use crate::request::object;
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

pub(super) fn combined_id(call_id: &str, item_id: &str) -> String {
    if item_id.is_empty() {
        call_id.to_owned()
    } else {
        format!("{call_id}|{item_id}")
    }
}

fn input_image(data: &str, mime_type: &str) -> Value {
    object([
        ("type", json!("input_image")),
        ("detail", json!("auto")),
        (
            "image_url",
            Value::String(format!("data:{mime_type};base64,{data}")),
        ),
    ])
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
                    Content::Image { data, mime_type } => Some(input_image(data, mime_type)),
                    _ => None,
                })
                .collect();
            (!parts.is_empty())
                .then(|| object([("role", json!("user")), ("content", Value::Array(parts))]))
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

fn orphan_tool_result(call_id: &str, content: &[Content]) -> Value {
    let text: String = yi_types::message::join_text(content, "\n");
    let output = if text.is_empty() {
        "(no tool output)"
    } else {
        text.as_str()
    };
    json!({
        "type": "message",
        "role": "assistant",
        "status": "completed",
        "content": [{
            "type": "output_text",
            "text": format!("Tool output for call_id {call_id} (no matching call):\n{output}"),
            "annotations": [],
        }],
    })
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
    let text: String = yi_types::message::join_text(content, "\n");
    let images: Vec<Value> = if vision {
        content
            .iter()
            .filter_map(|block| match block {
                Content::Image { data, mime_type } => Some(input_image(data, mime_type)),
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
        object([
            ("type", json!(kind)),
            ("call_id", json!(call_id)),
            ("output", Value::Array(parts)),
        ])
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
    let mut function_calls = std::collections::BTreeSet::new();
    let mut custom_tool_calls = std::collections::BTreeSet::new();
    let mut items = |messages: &[Cow<'_, AgentMessage>]| {
        let mut out = Vec::new();
        for message in messages {
            match message.as_ref() {
                AgentMessage::User { content, .. } => {
                    if let Some(item) = convert_user(content) {
                        out.push(item);
                    }
                }
                AgentMessage::Assistant { content, .. } => {
                    for item in convert_assistant(content, &freeform) {
                        let call_id = item.get("call_id").and_then(Value::as_str);
                        match item.get("type").and_then(Value::as_str) {
                            Some("function_call") => {
                                if let Some(call_id) = call_id {
                                    function_calls.insert(call_id.to_owned());
                                }
                            }
                            Some("custom_tool_call") => {
                                if let Some(call_id) = call_id {
                                    custom_tool_calls.insert(call_id.to_owned());
                                }
                            }
                            _ => {}
                        }
                        out.push(item);
                    }
                }
                AgentMessage::ToolResult {
                    tool_call_id,
                    tool_name,
                    content,
                    ..
                } => {
                    let is_freeform = freeform.contains(tool_name);
                    let (call_id, _) = call_and_item(tool_call_id);
                    let calls = if is_freeform {
                        &custom_tool_calls
                    } else {
                        &function_calls
                    };
                    if calls.contains(call_id) {
                        out.push(convert_tool_result(
                            tool_call_id,
                            content,
                            vision,
                            is_freeform,
                        ));
                    } else {
                        out.push(orphan_tool_result(call_id, content));
                    }
                }
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
pub(super) fn freeform_arguments(input: &str) -> Map<String, Value> {
    let mut arguments = Map::new();
    arguments.insert(
        FREEFORM_ARGUMENT.to_owned(),
        Value::String(input.to_owned()),
    );
    arguments
}

fn convert_tools(tools: &[ToolDef], strict: bool) -> Vec<Value> {
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
            None => {
                let closed = crate::schema::strict_tool_json(
                    &tool.parameters,
                    strict,
                    crate::schema::strict,
                );
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": closed.as_ref().unwrap_or(&tool.parameters),
                    "strict": closed.is_some(),
                })
            }
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
                params["reasoning"] = json!({"effort": mapped, "summary": "auto"});
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
    let mut params = object([
        ("model", json!(model.id)),
        ("input", Value::Array(input)),
        ("stream", json!(true)),
        ("store", json!(false)),
    ]);
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
    if let Some(tools) = &context.tools
        && !tools.is_empty()
    {
        params["tools"] = Value::Array(convert_tools(tools, crate::compat::strict_tools(model)));
    }
    if let Some(choice) = &context.tool_choice {
        params["tool_choice"] = convert_tool_choice(choice, context.tools.as_deref());
    }
    apply_reasoning(model, options, &mut params);
    // No explicit breakpoint is known to be accepted on this wire; the provider caches its own
    // prefix, and the per-request facts still render last (design §11).
    Encoded::provider_prefix(params, "input", transient)
}
