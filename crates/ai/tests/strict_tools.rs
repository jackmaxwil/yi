use std::error::Error;

use serde_json::{Map, Value, json};
use yi_ai::anthropic::{self, AnthropicOptions};
use yi_ai::openai::{self, ChunkMapper, OpenAiOptions};
use yi_ai::schema;
use yi_types::message::{AgentMessage, Content, UserContent};
use yi_types::model::{LlmContext, Model, ModelCost, ToolDef};

type TestResult = Result<(), Box<dyn Error>>;

fn model(id: &str, base_url: &str, compat: Option<Value>) -> Model {
    let zero = || serde_json::Number::from(0);
    Model {
        id: id.to_owned(),
        name: id.to_owned(),
        api: "a".to_owned(),
        provider: "p".to_owned(),
        base_url: base_url.to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 1_000,
        max_tokens: 100,
        compat,
        thinking_level_map: None,
        headers: None,
    }
}

/// The ten tool schemas a root session sends, as `request_budget`'s `tool_defs` built them.
fn yi_tools() -> Result<Vec<ToolDef>, Box<dyn Error>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/tool_schemas_2026-10-03.json");
    let tools: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    Ok(tools
        .into_iter()
        .map(|tool| ToolDef {
            name: tool["name"].as_str().unwrap_or_default().to_owned(),
            description: String::new(),
            parameters: tool["parameters"].clone(),
            freeform: None,
        })
        .collect())
}

fn asking(tools: Vec<ToolDef>) -> LlmContext {
    LlmContext {
        cache_ttl: yi_types::model::Ttl::Min5,
        system_prompt: "s".to_owned(),
        messages: vec![AgentMessage::user_input(
            UserContent::Text("q".to_owned()),
            0,
        )],
        transient: Vec::new(),
        schema: None,
        shared_through: None,
        reuse: yi_types::model::Reuse::Loop,
        tools: Some(tools),
        tool_choice: None,
    }
}

fn chat_strict(model: &Model, tools: Vec<ToolDef>) -> Vec<(String, bool)> {
    let params =
        openai::build_params(model, &asking(tools), &OpenAiOptions::default()).into_value();
    (params["tools"].as_array().into_iter().flatten())
        .map(|tool| {
            let function = &tool["function"];
            (
                function["name"].as_str().unwrap_or_default().to_owned(),
                function["strict"] == json!(true),
            )
        })
        .collect()
}

/// Dies with a tool sent strict in a shape OpenAI refuses, or with a tool left loose that the
/// converter could close; the judge is the existing response-format check, not the converter.
#[test]
fn every_closable_yi_tool_goes_to_openai_in_the_strict_shape() -> TestResult {
    let mut closed = Vec::new();
    for tool in yi_tools()? {
        if let Some(strict) = schema::strict_tool(&tool.parameters, true) {
            assert!(
                schema::strict(&strict),
                "{} converted to {strict}",
                tool.name
            );
            closed.push(tool.name);
        }
    }
    assert_eq!(
        closed,
        [
            "read",
            "edit",
            "write",
            "grep",
            "bash",
            "get_context",
            "ipython",
            "ask_user"
        ],
        "plan and todo carry free-form objects and stay loose"
    );
    let grep = yi_tools()?
        .into_iter()
        .find(|tool| tool.name == "grep")
        .ok_or("grep")?;
    let strict = schema::strict_tool(&grep.parameters, true).ok_or("grep closes")?;
    assert!(
        strict["properties"].get("pattern").is_some(),
        "a property named like a keyword stays"
    );
    Ok(())
}

/// Dies with a free-form object marked strict: there is no shape to close.
#[test]
fn a_free_form_object_is_never_strict() {
    let loose = json!({"type": "object", "properties": {"spec": {"type": "object"}}});
    assert_eq!(schema::strict_tool(&loose, true), None);
    assert_eq!(schema::strict_tool(&loose, false), None);
}

/// Dies with strict sent where the route cannot enforce it (a 400 on every request) or withheld
/// where it can.
#[test]
fn strict_follows_the_route_and_an_entry_can_opt_in() -> TestResult {
    let tools = || yi_tools().map(|tools| tools.into_iter().take(1).collect::<Vec<_>>());
    let cases = [
        (model("gpt-5.5", "https://api.openai.com/v1", None), true),
        (
            model("z-ai/glm-5.3-flash", "https://openrouter.ai/api/v1", None),
            false,
        ),
        (
            model(
                "z-ai/glm-5.3-flash",
                "https://openrouter.ai/api/v1",
                Some(json!({"strictTools": true})),
            ),
            true,
        ),
        (
            model(
                "gpt-5.5",
                "https://api.openai.com/v1",
                Some(json!({"strictTools": false})),
            ),
            false,
        ),
    ];
    for (model, strict) in cases {
        assert_eq!(
            chat_strict(&model, tools()?),
            [("read".to_owned(), strict)],
            "{}",
            model.base_url
        );
    }
    Ok(())
}

fn optional(name: &str, count: usize) -> ToolDef {
    let properties: Map<String, Value> = (0..count)
        .map(|index| (format!("p{index}"), json!({"type": "string"})))
        .collect();
    ToolDef {
        name: name.to_owned(),
        description: String::new(),
        parameters: json!({"type": "object", "properties": properties}),
        freeform: None,
    }
}

fn claude_strict(id: &str, tools: Vec<ToolDef>) -> Vec<bool> {
    let claude = model(id, "https://api.anthropic.com", None);
    let params =
        anthropic::build_params(&claude, &asking(tools), &AnthropicOptions::default()).into_value();
    (params["tools"].as_array().into_iter().flatten())
        .map(|tool| tool["strict"] == json!(true))
        .collect()
}

/// Dies with a request past Anthropic's 24 optional properties across strict tools, which it
/// refuses whole, or with a tool loosened while it still fit.
#[test]
fn anthropic_marks_tools_strict_up_to_its_caps_and_no_further() {
    assert_eq!(
        claude_strict("claude-opus-5-5", vec![optional("a", 20), optional("b", 4)]),
        [true, true]
    );
    assert_eq!(
        claude_strict(
            "claude-opus-5-5",
            vec![optional("a", 20), optional("b", 5), optional("c", 0)]
        ),
        [true, false, true]
    );
    assert_eq!(
        claude_strict("claude-3-7-sonnet", vec![optional("a", 1)]),
        [false]
    );
}

/// Dies with the session's own tools marked past Anthropic's caps: read, grep and bash spend
/// all 24 optional properties, so get_context and ask_user go loose and the free-form two stay.
#[test]
fn the_session_tools_fit_anthropic_caps_in_order() -> TestResult {
    let tools = yi_tools()?;
    let names: Vec<String> = tools.iter().map(|tool| tool.name.clone()).collect();
    let marked = names
        .into_iter()
        .zip(claude_strict("claude-opus-5-5", tools));
    let strict: Vec<String> = marked.filter(|(_, on)| *on).map(|(name, _)| name).collect();
    assert_eq!(strict, ["read", "edit", "write", "grep", "bash", "ipython"]);
    Ok(())
}

/// Dies with an OpenAI strict call's `null` reaching the tool, which reads it as a value; a
/// route without strict keeps what the model sent.
#[test]
fn a_strict_calls_nulls_reach_the_tool_as_absent() {
    let chunks = [
        json!({"id":"c","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":"{\"path\":\"a.rs\",\"offset\":null,"}}]}}]}),
        json!({"id":"c","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ranges\":[{\"from\":1,\"to\":null}]}"}}]}}]}),
        json!({"id":"c","choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
    ];
    let arguments = |model: &Model| {
        let mut mapper = ChunkMapper::new(model);
        for chunk in &chunks {
            let _ = mapper.push_chunk(chunk);
        }
        let mut found = Map::new();
        for event in mapper.finish() {
            if let yi_types::event::AssistantMessageEvent::Done { message, .. } = event
                && let AgentMessage::Assistant { content, .. } = message
            {
                for block in content {
                    if let Content::ToolCall { arguments, .. } = block {
                        found = arguments;
                    }
                }
            }
        }
        Value::Object(found)
    };
    assert_eq!(
        arguments(&model("gpt-5.5", "https://api.openai.com/v1", None)),
        json!({"path": "a.rs", "ranges": [{"from": 1}]})
    );
    assert_eq!(
        arguments(&model("glm", "https://openrouter.ai/api/v1", None)),
        json!({"path": "a.rs", "offset": null, "ranges": [{"from": 1, "to": null}]})
    );
}
