//! What the loop's tool table becomes on the Responses wire: a function tool, or a custom one.

use serde_json::{Value, json};
use yi_types::model::{ToolChoice, ToolDef};

pub(super) fn convert_tools(tools: &[ToolDef], strict: bool) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| match &tool.freeform {
            Some(format) => json!({
                "type": "custom",
                "name": tool.name,
                "description": tool.description,
                "format": {
                    "type": "grammar",
                    "syntax": "lark",
                    "definition": format.definition,
                },
            }),
            None => {
                let closed = crate::schema::strict_tool_json(
                    &tool.parameters,
                    strict,
                    crate::schema::strict,
                );
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": closed.as_ref().unwrap_or(&tool.parameters),
                    "strict": closed.is_some(),
                })
            }
        })
        .collect()
}

pub(super) fn convert_tool_choice(choice: &ToolChoice, tools: Option<&[ToolDef]>) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Tool(forced) => {
            let emitted_as_custom = tools.is_some_and(|tools| {
                tools
                    .iter()
                    .any(|tool| tool.name == forced.as_str() && tool.freeform.is_some())
            });
            let kind = if emitted_as_custom {
                "custom"
            } else {
                "function"
            };
            json!({"type": kind, "name": forced.as_str()})
        }
    }
}
