use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_types::kernel::{ExecuteResult, ExecuteStatus};

use crate::tool::{
    CancelFlag, Tool, ToolContext, ToolKind, ToolOutput, detail_text, error_output, require_str,
    text_output,
};

pub struct KernelCellOutcome {
    pub result: ExecuteResult,
    pub kernel_restarted: bool,
}

/// yi-runtime implements this over yi-kernel; yi-tools never depends on it.
pub trait KernelBridge: Send + Sync {
    fn execute_cell(&self, code: &str, cancelled: &CancelFlag)
    -> Result<KernelCellOutcome, String>;
}

pub struct IpythonTool {
    pub bridge: Arc<dyn KernelBridge>,
}

fn shell_cell(code: &str) -> Option<String> {
    let trimmed = code.trim_start();
    if let Some(body) = trimmed.strip_prefix("%%bash") {
        return Some(body.trim_start_matches(['\r', '\n']).to_owned());
    }
    let escaped: Vec<&str> = code
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix('!'))
        .collect();
    (!escaped.is_empty()).then(|| escaped.join("\n"))
}

impl Tool for IpythonTool {
    fn name(&self) -> &str {
        "ipython"
    }

    fn description(&self) -> &str {
        "Execute Python in the persistent agent kernel. Variables survive across calls; `await` is allowed at top level; `rlm` is preloaded."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "Python scratchpad code or `%%bash` shell cells to execute in the agent kernel. The kernel runs in Yi's own virtualenv, not the target project's: reach the project's environment through a `%%bash` cell that invokes the project's own interpreter or runner (`uv run`, `.venv/bin/python`, `cargo`, `npm`), and keep direct kernel imports for scratch work that does not depend on project packages."
                }
            },
            "required": ["code"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Exec
    }

    /// A shell cell reads through the same classifier as `bash`; Python is not
    /// statically readable, and the sandbox is what closes that residual.
    fn irreversible(&self, input: &Map<String, Value>) -> bool {
        let Some(code) = input.get("code").and_then(Value::as_str) else {
            return true;
        };
        match shell_cell(code) {
            Some(command) => yi_permission::verdict(&command) != yi_permission::Verdict::Allow,
            None => false,
        }
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let code = match require_str(&input, "code") {
            Ok(code) => code,
            Err(message) => return error_output(message),
        };
        let outcome = match self.bridge.execute_cell(code, &context.cancelled) {
            Ok(outcome) => outcome,
            Err(message) => return error_output(message),
        };
        let result = outcome.result;
        let mut sections = Vec::new();
        if !result.stdout.is_empty() {
            sections.push(result.stdout.clone());
        }
        if !result.stderr.is_empty() {
            sections.push(format!("stderr:\n{}", result.stderr));
        }
        if let Some(value) = &result.result {
            sections.push(value.clone());
        }
        if let Some(error) = &result.error {
            sections.push(if error.traceback.is_empty() {
                format!("{}: {}", error.ename, error.evalue)
            } else {
                error.traceback.join("\n")
            });
        }
        if result.status == ExecuteStatus::Aborted {
            sections.push("[cell aborted]".to_owned());
        }
        if outcome.kernel_restarted {
            sections.push("[IPython kernel was restarted; in-memory state was lost]".to_owned());
        }
        let text = if sections.is_empty() {
            "(no output)".to_owned()
        } else {
            sections.join("\n")
        };
        let mut output = text_output(text);
        // A kernel edit becomes a real patch here, where every other patch is
        // computed, rather than being reassembled by whoever renders it.
        let diffs: Vec<Value> = result
            .diffs
            .iter()
            .map(|diff| {
                let patch = crate::diff::patch(&diff.old_str, &diff.new_str, Path::new(&diff.path));
                json!({ "path": diff.path, "patch": detail_text(patch.as_str()) })
            })
            .collect();
        // The streams are carried apart from the joined text so a renderer can
        // style stderr and a traceback differently; the joined form stays the
        // model's view.
        output.result.details = json!({
            "status": result.status,
            "durationMs": result.duration_ms,
            "diffs": diffs,
            "attachments": result.attachments.len(),
            "sentAgentMessages": result.sent_agent_messages,
            "kernelRestarted": outcome.kernel_restarted,
            "code": detail_text(code),
            "stdout": detail_text(&result.stdout),
            "stderr": detail_text(&result.stderr),
            "result": detail_text(result.result.as_deref().unwrap_or_default()),
            "error": result.error,
        });
        output.is_error =
            result.status == ExecuteStatus::Error || result.status == ExecuteStatus::Aborted;
        output
    }
}
