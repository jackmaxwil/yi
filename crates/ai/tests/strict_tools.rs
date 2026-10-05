use std::error::Error;

use serde_json::{Map, Value, json};
use yi_ai::anthropic::{self, AnthropicOptions};
use yi_ai::openai::{self, OpenAiOptions};
use yi_ai::schema;
use yi_types::message::{AgentMessage, UserContent};
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

/// Dies with a tool that has an optional property sent strict to OpenAI, whose strict mode
/// makes every property required: in the #995 A/B the model then filled the extras with junk
/// (59% of calls refused, against 17% loose). Only a tool with nothing optional goes strict.
#[test]
fn openai_gets_strict_only_for_tools_with_nothing_optional() -> TestResult {
    let openai = model("gpt-5.5", "https://api.openai.com/v1", None);
    let strict: Vec<String> = chat_strict(&openai, yi_tools()?)
        .into_iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| name)
        .collect();
    assert_eq!(strict, ["edit", "write", "ipython"]);
    for tool in yi_tools()? {
        if let Some(closed) = schema::strict_tool(&tool.parameters) {
            assert_eq!(
                closed["additionalProperties"],
                json!(false),
                "{}",
                tool.name
            );
        }
    }
    let grep = yi_tools()?
        .into_iter()
        .find(|tool| tool.name == "grep")
        .ok_or("grep")?;
    let closed = schema::strict_tool(&grep.parameters).ok_or("grep closes")?;
    assert!(
        closed["properties"].get("pattern").is_some(),
        "a property named like a keyword stays"
    );
    Ok(())
}

fn responses_strict(model: &Model, tools: Vec<ToolDef>) -> Vec<(String, bool)> {
    let params =
        yi_ai::openai_responses::build_params(model, &asking(tools), &OpenAiOptions::default())
            .into_value();
    (params["tools"].as_array().into_iter().flatten())
        .map(|tool| {
            (
                tool["name"].as_str().unwrap_or_default().to_owned(),
                tool["strict"] == json!(true),
            )
        })
        .collect()
}

/// Dies with the Responses route strict where Chat Completions is not (or loose where it is):
/// the same ten tools go over both OpenAI routes and must mark the same three.
#[test]
fn responses_marks_the_same_tools_strict_as_chat_completions() -> TestResult {
    let openai = model("gpt-5.5", "https://api.openai.com/v1", None);
    let tools = yi_tools()?;
    assert_eq!(
        responses_strict(&openai, tools.clone()),
        chat_strict(&openai, tools)
    );
    let params = yi_ai::openai_responses::build_params(
        &openai,
        &asking(yi_tools()?),
        &OpenAiOptions::default(),
    )
    .into_value();
    for tool in params["tools"].as_array().into_iter().flatten() {
        if tool["strict"] == json!(true) {
            assert_eq!(
                tool["parameters"]["additionalProperties"],
                json!(false),
                "{}",
                tool["name"]
            );
        }
    }
    Ok(())
}

/// Dies with a free-form object marked strict: there is no shape to close.
#[test]
fn a_free_form_object_is_never_strict() {
    let loose = json!({"type": "object", "properties": {"spec": {"type": "object"}}});
    assert_eq!(schema::strict_tool(&loose), None);
}

/// Dies with strict sent where the route cannot enforce it (a 400 on every request) or withheld
/// where it can.
#[test]
fn strict_follows_the_route_and_an_entry_can_opt_in() -> TestResult {
    let tools = || {
        yi_tools().map(|tools| {
            tools
                .into_iter()
                .filter(|tool| tool.name == "write")
                .collect::<Vec<_>>()
        })
    };
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
            [("write".to_owned(), strict)],
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

/// Dies with a tool whose $defs hold the optional properties weight() used to skip: the caps
/// count them once the schema is closed, and past the cap Anthropic refuses the whole request.
#[test]
fn the_caps_count_properties_hidden_in_defs() {
    let optional: Map<String, Value> = (0..30)
        .map(|index| (format!("p{index}"), json!({"type": "string"})))
        .collect();
    let parameters = json!({
        "type": "object",
        "properties": {"x": {"type": "string"}},
        "required": ["x"],
        "$defs": {
            "big": {
                "type": "object",
                "properties": optional,
                "required": [],
            }
        },
    });
    let tools = vec![ToolDef {
        name: "big".to_owned(),
        description: String::new(),
        parameters,
        freeform: None,
    }];
    assert_eq!(claude_strict("claude-opus-5-5", tools), [false]);
}
