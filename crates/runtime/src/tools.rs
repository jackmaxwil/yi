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
    recovery_dir: Option<PathBuf>,
    auto_background: Option<std::time::Duration>,
    rules: Option<Arc<crate::rules::RuleEngine>>,
}

/// T19 tee target: the home root, never the user's working tree.
fn default_recovery_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".yi/tool-output"))
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
            recovery_dir: default_recovery_dir(),
            auto_background: None,
            rules: None,
        }
    }

    /// D13: default off — a command that detaches on its own is a surprise
    /// unless the user asked for it.
    pub fn with_auto_background(mut self, limit: Option<std::time::Duration>) -> Self {
        self.auto_background = limit;
        self
    }

    pub fn with_rules(mut self, rules: Option<Arc<crate::rules::RuleEngine>>) -> Self {
        self.rules = rules;
        self
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
            recovery_dir: self.recovery_dir.clone(),
            auto_background: self.auto_background,
        };
        let permission = self.permission.clone();
        let rules = self.rules.clone();
        let call_id = tool_call_id.to_owned();
        Box::pin(async move {
            // User rules gate before permission: a matching eligible gate rule
            // denies once with the rule body as evidence (D26 shape).
            if let Some(rules) = rules {
                let args_json = serde_json::to_string(&args).unwrap_or_default();
                if let Some(denial) = rules.check_tool(tool.name(), &args_json) {
                    return ToolOutcome {
                        result: error_tool_result(&denial),
                        is_error: true,
                    };
                }
            }
            if let Some(broker) = permission {
                let gate_tool = Arc::clone(&tool);
                let gate_args = args.clone();
                let gate_cwd = context.cwd.clone();
                let outcome = tokio::task::spawn_blocking(move || {
                    let preview = gate_tool.preview(&gate_args, &gate_cwd);
                    broker.decide_call(
                        gate_tool.name(),
                        gate_tool.kind(),
                        gate_tool.irreversible(&gate_args),
                        &call_id,
                        &gate_args,
                        preview.as_deref(),
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
