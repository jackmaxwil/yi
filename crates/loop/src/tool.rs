use std::future::Future;
use std::pin::Pin;

use serde_json::{Map, Value};
use yi_types::event::ToolResult;
use yi_types::model::ToolDef;

use crate::interrupt::InterruptSignal;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    pub result: ToolResult,
    pub is_error: bool,
}

pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = ToolOutcome> + Send + 'a>>;

pub trait AgentTool: Send + Sync {
    fn definition(&self) -> ToolDef;

    fn execution_mode(&self) -> crate::config::ExecutionMode {
        crate::config::ExecutionMode::Parallel
    }

    fn validate(&self, _args: &Map<String, Value>) -> Result<(), String> {
        Ok(())
    }

    fn execute<'a>(
        &'a self,
        tool_call_id: &'a str,
        args: Map<String, Value>,
        signal: &'a InterruptSignal,
    ) -> ToolFuture<'a>;
}

pub fn error_tool_result(message: &str) -> ToolResult {
    ToolResult {
        content: vec![yi_types::message::Content::Text {
            text: message.to_owned(),
            text_signature: None,
        }],
        details: Value::Object(Map::new()),
        usage: None,
        added_tool_names: None,
        terminate: None,
    }
}

/// The machine-readable half of a failure; the prose stays the model's view.
pub fn error_tool_result_kind(message: &str, kind: yi_types::event::ToolErrorKind) -> ToolResult {
    let mut result = error_tool_result(message);
    if let Value::Object(details) = &mut result.details {
        details.insert(
            "errorKind".to_owned(),
            Value::String(kind.as_str().to_owned()),
        );
    }
    result
}
