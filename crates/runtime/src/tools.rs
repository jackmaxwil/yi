use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};
use yi_loop::interrupt::InterruptSignal;
use yi_loop::tool::{ToolFuture, error_tool_result};
use yi_loop::{AgentTool, ToolOutcome};
use yi_tools::{CancelFlag, Tool, ToolContext};
use yi_types::model::ToolDef;

use crate::permission::PermissionBroker;

pub struct ToolAdapter {
    tool: Arc<dyn Tool>,
    cwd: PathBuf,
    cancelled: CancelFlag,
    permission: Option<Arc<PermissionBroker>>,
}

impl ToolAdapter {
    pub fn new(
        tool: Arc<dyn Tool>,
        cwd: PathBuf,
        cancelled: CancelFlag,
        permission: Option<Arc<PermissionBroker>>,
    ) -> Self {
        Self {
            tool,
            cwd,
            cancelled,
            permission,
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
        tool_call_id: &'a str,
        args: Map<String, Value>,
        _signal: &'a InterruptSignal,
    ) -> ToolFuture<'a> {
        let tool = Arc::clone(&self.tool);
        let context = ToolContext {
            cwd: self.cwd.clone(),
            cancelled: Arc::clone(&self.cancelled),
        };
        let permission = self.permission.clone();
        let call_id = tool_call_id.to_owned();
        Box::pin(async move {
            if let Some(broker) = permission {
                let gate_tool = Arc::clone(&tool);
                let gate_args = args.clone();
                let outcome = tokio::task::spawn_blocking(move || {
                    broker.decide_call(
                        gate_tool.name(),
                        gate_tool.kind(),
                        gate_tool.irreversible(&gate_args),
                        &call_id,
                        &gate_args,
                    )
                })
                .await;
                match outcome {
                    Ok(outcome) if outcome.allowed => {}
                    Ok(outcome) => {
                        return ToolOutcome {
                            result: error_tool_result(&format!(
                                "Permission denied: {}",
                                outcome.reason
                            )),
                            is_error: true,
                        };
                    }
                    Err(join_error) => {
                        return ToolOutcome {
                            result: error_tool_result(&format!(
                                "permission check failed: {join_error}"
                            )),
                            is_error: true,
                        };
                    }
                }
            }
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
