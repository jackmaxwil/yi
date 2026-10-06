use crate::common;

use std::error::Error;

use serde_json::{Map, Value, json};
use yi_ai::anthropic::{self, AnthropicOptions};
use yi_ai::openai::{self, OpenAiOptions};
use yi_ai::schema;
use yi_types::message::{AgentMessage, UserContent};
use yi_types::model::{LlmContext, Model, ToolDef};

type TestResult = Result<(), Box<dyn Error>>;

fn model(id: &str, base_url: &str, compat: Option<Value>) -> yi_types::model::Model {
    common::model(id, "a", "p", base_url, compat)
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
    marked(&params, |tool| &tool["function"])
}

fn marked(params: &Value, entry: impl Fn(&Value) -> &Value) -> Vec<(String, bool)> {
    (params["tools"].as_array().into_iter().flatten())
        .map(|tool| {
            let tool = entry(tool);
            (
                tool["name"].as_str().unwrap_or_default().to_owned(),
                tool["strict"] == json!(true),
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
    marked(&params, |tool| tool)
}

/// Dies with the Responses route marking a different set strict than Chat Completions, or
/// none at all: the same ten tools go over both OpenAI routes and must mark the same three.
#[test]
fn responses_marks_the_same_tools_strict_as_chat_completions() -> TestResult {
    let openai = model("gpt-5.5", "https://api.openai.com/v1", None);
    let tools = yi_tools()?;
    let responses = responses_strict(&openai, tools.clone());
    let strict: Vec<&str> = (responses.iter())
        .filter(|(_, on)| *on)
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(strict, ["edit", "write", "ipython"]);
    assert_eq!(responses, chat_strict(&openai, tools));
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

/// Dies with a tool strict where the provider 400s the whole request instead: a bare
/// `"type": "array"` node (no `items`) and a keyword outside strict mode's subset both pass
/// the old closure-only gate and reach OpenAI with `strict: true`, which refuses
/// "'items' must be defined for arrays" and unsupported keywords. None loosens the tool.
#[test]
fn strict_tool_refuses_what_strict_mode_refuses() {
    let bare_array = json!({
        "type": "object",
        "properties": {"tags": {"type": "array"}},
        "required": ["tags"],
    });
    assert_eq!(
        schema::strict_tool_json(&bare_array, true, schema::strict),
        None
    );
    let unknown_keyword = json!({
        "type": "object",
        "properties": {"x": {"type": "string"}},
        "required": ["x"],
        "examples": [{"x": "a"}],
    });
    assert_eq!(
        schema::strict_tool_json(&unknown_keyword, true, schema::strict),
        None
    );
    let refused_combinator = json!({
        "type": "object",
        "properties": {"x": {"type": "string"}},
        "required": ["x"],
        "discriminator": {"propertyName": "x"},
    });
    assert_eq!(
        schema::strict_tool_json(&refused_combinator, true, schema::strict),
        None
    );
}

/// Dies with a malformed `anyOf` or `enum` (an MCP tool's schema is third-party input) passed
/// through verbatim under `strict: true`, which 400s every request on that route: the anyOf
/// arm only matched arrays and `enum` was never checked, so both fell to the clone arm.
#[test]
fn a_malformed_anyof_or_enum_loosens_the_tool() {
    let anyof = json!({
        "type": "object",
        "properties": {"x": {"anyOf": 5}},
        "required": ["x"],
    });
    assert_eq!(schema::strict_tool_json(&anyof, true, schema::strict), None);
    let enumm = json!({
        "type": "object",
        "properties": {"x": {"enum": "red"}},
        "required": ["x"],
    });
    assert_eq!(schema::strict_tool_json(&enumm, true, schema::strict), None);
}

/// Dies with a malformed `required`, `properties` or `$defs` (third-party MCP input) cloned
/// through to Anthropic under `strict: true`, which refuses the schema and 400s the request.
#[test]
fn a_malformed_required_or_defs_loosens_the_tool_on_anthropic() {
    let x = json!({"x": {"type": "string"}});
    let schemas = [
        json!({"type": "object", "properties": x, "required": "x"}),
        json!({"type": "object", "properties": x, "required": [1]}),
        json!({"type": "object", "properties": x, "required": ["x"], "$defs": 5}),
        json!({"type": "object", "properties": x, "required": ["x"], "definitions": "d"}),
    ];
    for parameters in schemas {
        assert_eq!(schema::strict_tool(&parameters), None, "{parameters}");
        let tool = ToolDef {
            name: "mcp".to_owned(),
            description: String::new(),
            parameters,
            freeform: None,
        };
        assert_eq!(claude_strict("claude-opus-5-5", vec![tool]), [false]);
    }
}

/// Dies with a root schema that omits `"type": "object"` sent strict: strict routes want the
/// explicit type, so a typeless node with properties goes loose as it did before.
#[test]
fn a_typeless_root_with_properties_goes_loose() {
    let typeless = json!({"properties": {"x": {"type": "string"}}, "required": ["x"]});
    assert_eq!(schema::strict_tool(&typeless), None);
    let tool = ToolDef {
        name: "mcp".to_owned(),
        description: String::new(),
        parameters: typeless,
        freeform: None,
    };
    assert_eq!(claude_strict("claude-opus-5-5", vec![tool]), [false]);
}

/// Dies with the open-map pattern (`"additionalProperties": {"type": "string"}`) coerced to
/// `false`: strict mode can only express the closed form, so the tool loosens and keeps its
/// contract (extra keys allowed, must be strings) rather than silently forbidding them.
#[test]
fn a_schema_valued_additional_properties_loosens_the_tool() {
    let open_map = json!({
        "type": "object",
        "properties": {"name": {"type": "string"}},
        "required": ["name"],
        "additionalProperties": {"type": "string"},
    });
    assert_eq!(
        schema::strict_tool_json(&open_map, true, schema::strict),
        None
    );
}
