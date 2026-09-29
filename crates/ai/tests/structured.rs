use serde_json::{Value, json};
use yi_ai::anthropic::{self, AnthropicOptions};
use yi_ai::openai::{self, OpenAiOptions};
use yi_ai::openai_responses;
use yi_types::message::{AgentMessage, UserContent};
use yi_types::model::{LlmContext, Model, ModelCost};

fn model() -> Model {
    let zero = || serde_json::Number::from(0);
    Model {
        id: "m".to_owned(),
        name: "m".to_owned(),
        api: "a".to_owned(),
        provider: "p".to_owned(),
        base_url: "https://example.invalid".to_owned(),
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
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn asking(schema: Value) -> LlmContext {
    LlmContext {
        cache_ttl: yi_types::model::Ttl::Min5,
        system_prompt: "s".to_owned(),
        messages: vec![AgentMessage::host_user(
            UserContent::Text("q".to_owned()),
            0,
        )],
        transient: Vec::new(),
        schema: Some(schema),
        shared_through: None,
        reuse: yi_types::model::Reuse::Loop,
        tools: None,
        tool_choice: None,
    }
}

fn closed() -> Value {
    json!({"type": "object", "properties": {"answer": {"type": "string"}},
           "required": ["answer"], "additionalProperties": false})
}

#[test]
fn a_closed_schema_goes_to_every_provider_strict() {
    let context = asking(closed());
    let claude =
        anthropic::build_params(&model(), &context, &AnthropicOptions::default()).into_value();
    assert_eq!(
        claude["output_config"]["format"],
        json!({"type": "json_schema", "schema": closed()})
    );
    let chat = openai::build_params(&model(), &context, &OpenAiOptions::default()).into_value();
    assert_eq!(
        chat["response_format"]["json_schema"]["strict"],
        json!(true)
    );
    assert_eq!(chat["response_format"]["json_schema"]["schema"], closed());
    let responses = openai_responses::build_params(&model(), &context, &OpenAiOptions::default());
    assert_eq!(responses["text"]["format"]["strict"], json!(true));
}

#[test]
fn an_open_schema_is_guidance_where_strict_mode_would_refuse_it() {
    let open = json!({"type": "object", "required": ["answer"]});
    let context = asking(open.clone());
    let claude =
        anthropic::build_params(&model(), &context, &AnthropicOptions::default()).into_value();
    assert!(claude.get("output_config").is_none(), "{claude}");
    let chat = openai::build_params(&model(), &context, &OpenAiOptions::default()).into_value();
    assert_eq!(
        chat["response_format"]["json_schema"]["strict"],
        json!(false)
    );
    assert!(!yi_ai::schema::strict(
        &json!({"type": "string", "maxLength": 3})
    ));
}

#[test]
fn strict_judges_the_schema_nodes_not_the_property_names() {
    let strict = yi_ai::schema::strict;
    let open_without_type =
        json!({"properties": {"line": {"type": "integer"}}, "required": ["line"]});
    let open_nullable = json!({"type": "object", "properties": {"hit": {
        "type": ["object", "null"], "properties": {"line": {"type": "integer"}}, "required": ["line"]}},
        "required": ["hit"], "additionalProperties": false});
    let array_root = json!({"type": "array", "items": {"type": "string"}});
    let combined = json!({"type": "object", "properties": {}, "required": [],
        "additionalProperties": false, "allOf": []});
    let one_of = json!({"type": "object", "properties": {"a": {"oneOf": [{"type": "object",
        "properties": {"x": {"type": "string"}}}]}}, "required": ["a"], "additionalProperties": false});
    for refused in [
        &open_without_type,
        &open_nullable,
        &array_root,
        &combined,
        &one_of,
    ] {
        assert!(!strict(refused), "{refused}");
    }
    let named_like_keywords = json!({"type": "object",
        "properties": {"pattern": {"type": "string"}, "minimum": {"type": "integer"}},
        "required": ["pattern", "minimum"], "additionalProperties": false});
    assert!(strict(&named_like_keywords));
    let nested = json!({"type": "object", "properties": {"hits": {"type": "array", "items": {
        "type": "object", "properties": {"line": {"type": "integer"}}, "required": ["line"],
        "additionalProperties": false}}}, "required": ["hits"], "additionalProperties": false});
    assert!(strict(&nested));
}
