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
    wall: crate::wall::Wall,
    ext: Option<crate::session::ExtHook>,
    rejections: std::sync::Mutex<std::collections::BTreeMap<String, u32>>,
}

fn files_matched(name: &str, text: &str) -> u32 {
    if name != "grep" && name != "glob" {
        return 0;
    }
    let mut files: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for line in text.lines().take(512) {
        let path = line.split_once(':').map_or(line, |(path, _)| path);
        if !path.is_empty() && !path.starts_with('[') {
            files.insert(path);
        }
    }
    u32::try_from(files.len()).unwrap_or(u32::MAX)
}

fn exit_of(result: &yi_types::event::ToolResult) -> Option<i32> {
    result
        .details
        .get("exitCode")
        .and_then(Value::as_i64)
        .and_then(|code| i32::try_from(code).ok())
}

fn result_text(result: &yi_types::event::ToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            yi_types::message::Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn grid_note(tool: &str, command: &str, result: &yi_types::event::ToolResult) -> Option<String> {
    if tool != "bash" || !command.trim_start().starts_with("grid ") {
        return None;
    }
    let text = result_text(result);
    let body = text.trim();
    (body.is_empty() || body.lines().count() <= 1 && body.contains("exit code"))
        .then(crate::affordance::grid_empty)
        .flatten()
}

fn absolute(cwd: &std::path::Path, path: &str) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    }
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
            wall: crate::wall::Wall::default(),
            ext: None,
            rejections: std::sync::Mutex::new(std::collections::BTreeMap::new()),
        }
    }

    pub fn with_extensions(mut self, ext: Option<crate::session::ExtHook>) -> Self {
        self.ext = ext;
        self
    }

    /// D13: default off — a command that detaches on its own is a surprise
    /// unless the user asked for it.
    pub fn with_auto_background(mut self, limit: Option<std::time::Duration>) -> Self {
        self.auto_background = limit;
        self
    }

    pub fn with_wall(mut self, wall: crate::wall::Wall) -> Self {
        self.wall = wall;
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
        let Err(reason) = self.tool.validate(args) else {
            return Ok(());
        };
        let repeated = self
            .rejections
            .lock()
            .map(|mut seen| {
                let count = seen.entry(reason.clone()).or_insert(0);
                *count = count.saturating_add(1);
                *count > 1
            })
            .unwrap_or(false);
        if !repeated {
            return Err(reason);
        }
        Err(format!(
            "{reason}\n{}",
            crate::affordance::call_template(self.tool.name(), &self.tool.schema(), args)
        ))
    }

    fn execute<'a>(
        &'a self,
        tool_call_id: &'a str,
        args: Map<String, Value>,
        _signal: &'a InterruptSignal,
    ) -> ToolFuture<'a> {
        let tool = Arc::clone(&self.tool);
        let mut context = ToolContext {
            cwd: self.cwd.clone(),
            cancelled: Arc::clone(&self.cancelled),
            recovery_dir: self.recovery_dir.clone(),
            auto_background: self.auto_background,
            sandbox: None,
        };
        let permission = self.permission.clone();
        let rules = self.rules.clone();
        let wall = self.wall.clone();
        let ext = self.ext.clone();
        let call_id = tool_call_id.to_owned();
        Box::pin(async move {
            if let Some(ext) = &ext {
                ext(crate::ext::Event::ToolCall {
                    name: tool.name().to_owned(),
                    target: args
                        .get("path")
                        .and_then(Value::as_str)
                        .map(|path| absolute(&context.cwd, path)),
                });
            }
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
            if let Some(denial) = wall.check(tool.name(), tool.kind(), &args, &context.cwd) {
                return ToolOutcome {
                    result: error_tool_result(&denial),
                    is_error: true,
                };
            }
            let mut contained: Option<(Arc<PermissionBroker>, String)> = None;
            if let Some(broker) = permission {
                let sandbox = broker.sandbox().cloned();
                let reporter = Arc::clone(&broker);
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
                    Ok(outcome) if outcome.allowed => {
                        if outcome.contained {
                            context.sandbox = sandbox;
                            contained = Some((reporter, outcome.identity));
                        }
                    }
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
            let name = tool.name().to_owned();
            let command = args
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let output = tokio::task::spawn_blocking(move || tool.execute(args, &context)).await;
            match output {
                Ok(mut output) => {
                    // A contained command the sandbox refused asks the next
                    // time, rather than failing the same way forever.
                    if let Some((broker, identity)) = &contained
                        && yi_tools::denial_hint(
                            exit_of(&output.result),
                            &result_text(&output.result),
                        )
                        .is_some()
                    {
                        broker.note_containment_failure(identity);
                    }
                    if let Some(line) = grid_note(&name, &command, &output.result) {
                        crate::affordance::append(&mut output.result, &line);
                    }
                    if let Some(ext) = &ext {
                        let text = result_text(&output.result);
                        ext(crate::ext::Event::ToolResult {
                            files_matched: files_matched(&name, &text),
                            exit: output
                                .result
                                .details
                                .get("exitCode")
                                .and_then(Value::as_i64)
                                .and_then(|code| i32::try_from(code).ok())
                                .or(Some(i32::from(output.is_error))),
                            name,
                        });
                    }
                    ToolOutcome {
                        result: output.result,
                        is_error: output.is_error,
                    }
                }
                Err(join_error) => ToolOutcome {
                    result: error_tool_result(&format!("tool execution failed: {join_error}")),
                    is_error: true,
                },
            }
        })
    }
}
