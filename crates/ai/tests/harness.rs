//! Helpers the AI crate's integration tests share.

use std::error::Error;

use serde_json::Value;
use yi_types::model::LlmContext;

pub type Res = Result<(), Box<dyn Error>>;

/// Drain a stream to its last event on a current-thread runtime.
pub fn drain(
    open: impl FnOnce() -> tokio::sync::mpsc::Receiver<yi_types::event::AssistantMessageEvent>,
) -> Res {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut receiver = open();
        while receiver.recv().await.is_some() {}
    });
    Ok(())
}

/// A loop request with one user turn and a bash tool: the shape every mapper test drives.
pub fn loop_context(parameters: Value) -> LlmContext {
    LlmContext {
        system_prompt: "be terse".to_owned(),
        messages: vec![yi_types::message::AgentMessage::user_input(
            yi_types::message::UserContent::Text("hi".to_owned()),
            0,
        )],
        reuse: yi_types::model::Reuse::Loop,
        transient: Vec::new(),
        schema: None,
        shared_through: None,
        cache_ttl: yi_types::model::Ttl::Min5,
        cache_tail: true,
        tools: Some(vec![yi_types::model::ToolDef {
            name: "bash".to_owned(),
            description: "run".to_owned(),
            parameters,
            freeform: None,
        }]),
        tool_choice: None,
    }
}

/// A loop request with no messages and no tools: the shape a header or auth test sends.
pub fn empty_context() -> LlmContext {
    LlmContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        reuse: yi_types::model::Reuse::Loop,
        transient: Vec::new(),
        schema: None,
        shared_through: None,
        cache_ttl: yi_types::model::Ttl::Min5,
        cache_tail: true,
        tools: None,
        tool_choice: None,
    }
}
