use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::process::{OUTPUT_CAP, run_captured};
use crate::tool::{Tool, ToolContext, ToolKind, ToolOutput, error_output, text_output};

pub struct ExecTool {
    path: PathBuf,
    name: String,
    description: String,
    schema: Value,
    kind: ToolKind,
}

fn parse_kind(value: Option<&str>) -> ToolKind {
    match value {
        Some("read") => ToolKind::Read,
        Some("write") => ToolKind::Write,
        _ => ToolKind::Exec,
    }
}

impl ExecTool {
    fn from_schema_output(path: PathBuf, output: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(output).ok()?;
        let name = value.get("name")?.as_str()?.to_owned();
        let description = value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let schema = value
            .get("input_schema")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        let kind = parse_kind(value.get("kind").and_then(Value::as_str));
        Some(Self {
            path,
            name,
            description,
            schema,
            kind,
        })
    }
}

impl Tool for ExecTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> Value {
        self.schema.clone()
    }

    fn kind(&self) -> ToolKind {
        self.kind
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let args = match serde_json::to_vec(&Value::Object(input)) {
            Ok(args) => args,
            Err(error) => return error_output(format!("failed to encode arguments: {error}")),
        };
        let mut process = crate::process::command(&self.path);
        process.current_dir(&context.cwd);
        let capture = match run_captured(process, Some(args), &context.cancelled, OUTPUT_CAP) {
            Ok(capture) => capture,
            Err(message) => return error_output(message),
        };
        if capture.cancelled {
            return error_output("[tool aborted]");
        }
        if capture.exit_code != Some(0) {
            let message = if capture.stderr.is_empty() {
                format!("tool exited with code {:?}", capture.exit_code)
            } else {
                capture.stderr
            };
            return error_output(message);
        }
        text_output(capture.stdout)
    }
}

/// User-level only. Project `.yi/tools` waits on hash-pinned trust (D30): without a grant
/// store, a cloned repo's tools are arbitrary code behind one prompt.
pub fn discover_exec_tools(dir: &Path) -> Vec<ExecTool> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut tools = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_executable_file(&path) {
            continue;
        }
        let mut command = crate::process::command(&path);
        command.arg("--schema");
        let never: crate::tool::CancelFlag = std::sync::Arc::new(|| false);
        let Ok(capture) = run_captured(command, None, &never, OUTPUT_CAP) else {
            continue;
        };
        if capture.exit_code != Some(0) {
            continue;
        }
        if let Some(tool) = ExecTool::from_schema_output(path, &capture.stdout) {
            tools.push(tool);
        }
    }
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    tools
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_file())
        .unwrap_or(false)
}
