//! The smallest model and context several mapper suites were each spelling out, now in one place.
use serde_json::json;
use yi_types::message::{AgentMessage, UserContent};
use yi_types::model::{LlmContext, Model, ModelCost, ToolDef};

pub fn plain() -> Model {
    Model {
        id: "m".to_owned(),
        name: "m".to_owned(),
        api: "a".to_owned(),
        provider: "p".to_owned(),
        base_url: "https://example.invalid".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost::free(),
        context_window: 1_000,
        max_tokens: 100,
        compat: None,
        thinking_level_map: None,
        headers: None,
        read_timeout_secs: None,
    }
}

pub fn plain_context() -> LlmContext {
    LlmContext {
        cache_ttl: yi_types::model::Ttl::Min5,
        system_prompt: "be terse".to_owned(),
        messages: vec![AgentMessage::user_input(
            UserContent::Text("hi".to_owned()),
            0,
        )],
        transient: Vec::new(),
        schema: None,
        shared_through: None,
        reuse: yi_types::model::Reuse::Loop,
        tools: Some(vec![ToolDef {
            name: "bash".to_owned(),
            description: "run".to_owned(),
            parameters: json!({"type":"object","properties":{"cmd":{"type":"string"}}}),
            freeform: None,
        }]),
        tool_choice: None,
    }
}
