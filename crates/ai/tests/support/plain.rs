//! The smallest model and context several mapper suites were each spelling out, now in one place.
use serde_json::json;
use yi_types::message::{AgentMessage, UserContent};
use yi_types::model::{LlmContext, Model, ToolDef};

pub fn plain() -> Model {
    crate::common::model("m", "a", "p", "https://example.invalid", None)
}

pub fn plain_context() -> LlmContext {
    LlmContext {
        cache_ttl: yi_types::model::Ttl::Min5,
        cache_tail: true,
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
