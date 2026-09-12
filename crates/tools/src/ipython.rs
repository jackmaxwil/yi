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
        "Execute Python in this session's kernel: one process, yours alone, that boots on the first cell and keeps its variables across your calls and across compaction (a snapshot revives them; a restart after a hang says so and starts empty). Nothing here is shared with bash; the cwd is. `await` works at top level; `rlm` is preloaded (`help(rlm.run)`); `%%bash` runs a shell in the kernel's env and `%pip install x` adds a package; pandas (with openpyxl) reads spreadsheets, and `anydoc` and `pdf_inspector` (`extract_text`) read documents read cannot show. Output over 64 KiB is cut; a cell is interrupted after 600 s, so split longer work across cells. Children run their own kernels: `rlm.status()` shows them, `rlm.put/get` and `kernel://<name>/<var>` move objects between kernels whole, and a child reads yours through `kernel://main/<var>`."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "Python scratchpad code or `%%bash` shell cells to execute in the agent kernel. The kernel is Yi's own venv over the machine's site-packages: what `python3` here imports, a cell imports; a project's own venv is reached through a `%%bash` cell that runs its interpreter or runner (`uv run`, `.venv/bin/python`, `cargo`, `npm`)."
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
        cell_output(code, outcome)
    }
}

/// The distribution a `ModuleNotFoundError` names, top-level only: `a.b` installs as `a`.
fn missing_module(evalue: &str) -> Option<&str> {
    let rest = evalue.strip_prefix("No module named '")?;
    let name = rest.split('\'').next()?.split('.').next()?;
    (!name.is_empty()).then_some(name)
}

pub fn cell_output(code: &str, outcome: KernelCellOutcome) -> ToolOutput {
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
        if error.ename == "ModuleNotFoundError"
            && let Some(name) = missing_module(&error.evalue)
        {
            sections.push(format!(
                "`{name}` is not installed in the kernel. Run `%pip install {name}` in a cell; `pip` in bash installs into a different Python."
            ));
        }
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
    // The streams are carried apart from the joined text so a renderer can style stderr
    // and a traceback differently; the joined form stays the model's view.
    output.result.details = json!({
        "status": result.status,
        "durationMs": result.duration_ms,
        "diffs": diffs,
        "attachments": result.attachments.len(),
        "attachmentMedia": result.attachments,
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

#[cfg(test)]
mod tests {
    use super::missing_module;

    #[test]
    fn a_missing_module_names_its_distribution() {
        assert_eq!(missing_module("No module named 'xlrd'"), Some("xlrd"));
        assert_eq!(
            missing_module("No module named 'yaml.loader'"),
            Some("yaml")
        );
        assert_eq!(missing_module("name 'x' is not defined"), None);
    }
}
