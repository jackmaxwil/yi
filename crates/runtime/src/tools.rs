use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_loop::interrupt::InterruptSignal;
use yi_loop::tool::{ToolFuture, error_tool_result};
use yi_loop::{AgentTool, ToolOutcome};
use yi_tools::{CancelFlag, Tool, ToolContext};
use yi_types::model::ToolDef;

pub struct ToolAdapter {
    tool: Arc<dyn Tool>,
    cwd: PathBuf,
    cancelled: CancelFlag,
}

impl ToolAdapter {
    pub fn new(tool: Arc<dyn Tool>, cwd: PathBuf, cancelled: CancelFlag) -> Self {
        Self {
            tool,
            cwd,
            cancelled,
        }
    }
}

impl AgentTool for ToolAdapter {
    fn definition(&self) -> ToolDef {
        ToolDef {
            name: self.tool.name().to_owned(),
            description: self.tool.description().to_owned(),
            parameters: self.tool.schema(),
        }
    }

    fn validate(&self, args: &Map<String, Value>) -> Result<(), String> {
        self.tool.validate(args)
    }

    fn execute<'a>(
        &'a self,
        _tool_call_id: &'a str,
        args: Map<String, Value>,
        _signal: &'a InterruptSignal,
    ) -> ToolFuture<'a> {
        let tool = Arc::clone(&self.tool);
        let context = ToolContext {
            cwd: self.cwd.clone(),
            cancelled: Arc::clone(&self.cancelled),
        };
        Box::pin(async move {
            let output = tokio::task::spawn_blocking(move || tool.execute(args, &context)).await;
            match output {
                Ok(output) => ToolOutcome {
                    result: output.result,
                    is_error: output.is_error,
                },
                Err(join_error) => ToolOutcome {
                    result: error_tool_result(&format!("tool execution failed: {join_error}")),
                    is_error: true,
                },
            }
        })
    }
}
