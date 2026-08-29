use std::error::Error;

use serde_json::{Map, Value, json};
use yi_ai::anthropic::{AnthropicOptions, Thinking, build_params};
use yi_ai::faux::{faux_assistant_message, faux_tool_call};
use yi_ai::openai::{self, OpenAiOptions};
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

fn openai_model() -> Model {
    Model {
        id: "gpt-5".to_owned(),
        name: "GPT-5".to_owned(),
        api: "openai-completions".to_owned(),
        provider: "openai".to_owned(),
        base_url: "https://api.openai.com/v1".to_owned(),
        ..model()
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
        "{}\n{}\n{}",
        identity_fragment(),
        yi_runtime::doctrine_fragment(),
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

/// Every message the second turn inherits, serialized as the first turn sent
/// it. The breakpoint marker is dropped: it moves to the newest message every
/// turn and the documented cache key excludes it.
fn message_prefix(params: &Value) -> Result<Vec<String>, Box<dyn Error>> {
    let mut messages = params
        .get("messages")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    strip_cache_control(&mut messages);
    let items = messages.as_array().ok_or("messages is not an array")?;
    items
        .iter()
        .map(|message| Ok(serde_json::to_string(message)?))
        .collect()
}

/// A provider caches on an exact content prefix, so a request that re-renders
/// anything the previous turn already sent silently re-bills the whole
/// conversation while producing identical output — invisible to every other
/// gate. This is what D51 was.
fn assert_prefix_survives_a_turn(first: &Value, second: &Value) -> TestResult {
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
    let (before, after) = (message_prefix(first)?, message_prefix(second)?);
    assert!(
        after.len() > before.len(),
        "the second turn must carry the first turn's messages plus its own"
    );
    for (index, sent) in before.iter().enumerate() {
        assert_eq!(
            Some(sent),
            after.get(index),
            "message {index} was re-rendered by a later turn (D51)"
        );
    }
    Ok(())
}

#[test]
fn the_anthropic_cached_prefix_survives_a_turn() -> TestResult {
    let model = model();
    let options = options();
    assert_prefix_survives_a_turn(
        &build_params(&model, &context(first_turn()), &options),
        &build_params(&model, &context(second_turn()), &options),
    )
}

/// OpenAI caches automatically on the same content prefix, and carries the
/// system prompt as the first message rather than its own field, so the same
/// check covers one more block there.
#[test]
fn the_openai_cached_prefix_survives_a_turn() -> TestResult {
    let model = openai_model();
    let options = OpenAiOptions {
        session_id: Some("session-1".to_owned()),
        ..OpenAiOptions::default()
    };
    assert_prefix_survives_a_turn(
        &openai::build_params(&model, &context(first_turn()), &options),
        &openai::build_params(&model, &context(second_turn()), &options),
    )
}

/// The invariant the whole cache layout rests on: with no attach between two
/// requests, every cached block is byte-identical, and an attach rebuilds only
/// the block it landed in. The universal prefix (block 0) never moves, which is
/// what a fan-out of children reads.
#[test]
fn the_assembled_prefix_is_stable_across_a_turn_and_a_yard_change() -> TestResult {
    use yi_runtime::ext::{PromptState, Rank, Slot, Trust};
    use yi_types::model::SYSTEM_BLOCK_SEPARATOR;

    let mut state = PromptState::new("cafe1234".to_owned());
    state.attach(
        Slot::new(Rank::Identity, "identity"),
        identity_fragment().to_owned(),
    );
    state.attach(
        Slot::new(Rank::Doctrine, "doctrine"),
        yi_runtime::doctrine_fragment().to_owned(),
    );
    state.attach(
        Slot::new(Rank::Mode, "permission"),
        mode_fragment(PermissionMode::Auto).to_owned(),
    );
    state.attach_external("AGENTS.md", Trust::Untrusted, "run the repo's own gate");

    let blocks = |state: &PromptState| -> Vec<String> {
        state
            .assemble()
            .split(SYSTEM_BLOCK_SEPARATOR)
            .map(str::to_owned)
            .collect()
    };
    let before = blocks(&state);
    assert_eq!(before.len(), 3, "universal, trusted, yard");
    assert_eq!(
        before,
        blocks(&state),
        "assembling the same state twice must produce the same bytes"
    );

    state.attach_external("AGENTS.md", Trust::Untrusted, "the repository changed");
    let after_yard = blocks(&state);
    assert_eq!(before[0], after_yard[0], "block 0 survives a yard change");
    assert_eq!(before[1], after_yard[1], "block 1 survives a yard change");
    assert_ne!(before[2], after_yard[2]);

    state.attach(Slot::new(Rank::Protocol, "orchestrate"), "PLAN".to_owned());
    let after_attach = blocks(&state);
    assert_eq!(
        before[0], after_attach[0],
        "a mid-session attach never rebuilds the universal prefix"
    );
    assert_ne!(before[1], after_attach[1]);
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
