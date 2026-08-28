use std::error::Error;

use serde_json::{Map, Value, json};
use yi_ai::anthropic::{AnthropicOptions, Thinking, build_params};
use yi_ai::faux::{faux_assistant_message, faux_tool_call};
use yi_runtime::{PermissionMode, builtin_tools, identity_fragment, mode_fragment};
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
use yi_types::model::{LlmContext, Model, ModelCost, ToolDef};

type TestResult = Result<(), Box<dyn Error>>;

/// A fixed model, not a catalog lookup: the budget measures Yi's own prompt and
/// tool table, and must not move when a bundled model's metadata changes.
fn model() -> Model {
    let zero = || serde_json::Number::from(0u64);
    Model {
        id: "claude-opus-4-5".to_owned(),
        name: "Opus".to_owned(),
        api: "anthropic-messages".to_owned(),
        provider: "anthropic".to_owned(),
        base_url: "https://api.anthropic.com".to_owned(),
        reasoning: true,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 200_000,
        max_tokens: 32_000,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn options() -> AnthropicOptions {
    AnthropicOptions {
        thinking: Thinking::Off,
        cache: true,
        ..AnthropicOptions::default()
    }
}

fn tool_defs() -> Vec<ToolDef> {
    builtin_tools()
        .iter()
        .map(|tool| ToolDef {
            name: tool.name().to_owned(),
            description: tool.description().to_owned(),
            parameters: tool.schema(),
        })
        .collect()
}

/// The skills catalog is deliberately excluded: it is assembled from the
/// machine's global and project roots, so including it would make the budget
/// depend on what the runner has installed.
fn system_prompt() -> String {
    format!(
        "{}\n{}",
        identity_fragment(),
        mode_fragment(PermissionMode::Ask)
    )
}

fn user(text: &str) -> AgentMessage {
    AgentMessage::User {
        content: UserContent::Text(text.to_owned()),
        timestamp: 0,
    }
}

fn assistant_call(id: &str, name: &str, arguments: Map<String, Value>) -> AgentMessage {
    faux_assistant_message(
        vec![faux_tool_call(id, name, arguments)],
        StopReason::ToolUse,
    )
}

fn tool_result(id: &str, name: &str, text: &str) -> AgentMessage {
    AgentMessage::ToolResult {
        tool_call_id: id.to_owned(),
        tool_name: name.to_owned(),
        content: vec![Content::Text {
            text: text.to_owned(),
            text_signature: None,
        }],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 0,
    }
}

fn context(messages: Vec<AgentMessage>) -> LlmContext {
    LlmContext {
        system_prompt: system_prompt(),
        messages,
        tools: Some(tool_defs()),
    }
}

fn first_turn() -> Vec<AgentMessage> {
    vec![user("read src/lib.rs and tell me what it exports")]
}

fn second_turn() -> Vec<AgentMessage> {
    let mut arguments = Map::new();
    arguments.insert("path".to_owned(), json!("src/lib.rs"));
    let mut messages = first_turn();
    messages.push(assistant_call("call-1", "read", arguments));
    messages.push(tool_result("call-1", "read", "pub mod advisor;"));
    messages.push(user("now check the tests"));
    messages
}

fn prefix_bytes(params: &Value) -> Result<usize, Box<dyn Error>> {
    let system = serde_json::to_string(params.get("system").unwrap_or(&Value::Null))?;
    let tools = serde_json::to_string(params.get("tools").unwrap_or(&Value::Null))?;
    Ok(system.len().saturating_add(tools.len()))
}

/// The breakpoint moves to the newest message every turn and is not part of
/// the prefix hash, so it is the one key a stability check must ignore.
fn strip_cache_control(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("cache_control");
            for nested in map.values_mut() {
                strip_cache_control(nested);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_cache_control(item);
            }
        }
        _ => {}
    }
}

/// A provider caches on an exact byte prefix, so anything that rewrites the
/// system block or the tool table between turns silently multiplies cost while
/// producing identical output — invisible to every other gate.
#[test]
fn the_cached_prefix_is_byte_identical_across_turns() -> TestResult {
    let model = model();
    let options = options();
    let first = build_params(&model, &context(first_turn()), &options);
    let second = build_params(&model, &context(second_turn()), &options);

    assert_eq!(
        serde_json::to_string(first.get("system").unwrap_or(&Value::Null))?,
        serde_json::to_string(second.get("system").unwrap_or(&Value::Null))?,
        "the system block must not change between turns"
    );
    assert_eq!(
        serde_json::to_string(first.get("tools").unwrap_or(&Value::Null))?,
        serde_json::to_string(second.get("tools").unwrap_or(&Value::Null))?,
        "the tool table must not change between turns"
    );
    let opening = |params: &Value| -> Result<String, Box<dyn Error>> {
        let mut message = params
            .get("messages")
            .and_then(|messages| messages.as_array())
            .and_then(|messages| messages.first())
            .cloned()
            .unwrap_or(Value::Null);
        strip_cache_control(&mut message);
        Ok(serde_json::to_string(&message)?)
    };
    assert_eq!(
        opening(&first)?,
        opening(&second)?,
        "the opening user message must not be rewritten by a later turn (D51)"
    );
    Ok(())
}

/// Read by scripts/guardrails/check_request_budget.py, which owns the ratchet.
#[test]
fn report_the_prefix_size() -> TestResult {
    let params = build_params(&model(), &context(first_turn()), &options());
    let system = serde_json::to_string(params.get("system").unwrap_or(&Value::Null))?;
    let tools = serde_json::to_string(params.get("tools").unwrap_or(&Value::Null))?;
    println!(
        "REQUEST_PREFIX system={} tools={} total={}",
        system.len(),
        tools.len(),
        prefix_bytes(&params)?
    );
    Ok(())
}
