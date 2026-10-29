use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::process::{OUTPUT_CAP, run_captured, run_captured_until};
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
        // An exec tool declares only its input; its value is whatever JSON it prints.
        let value = (context.program).then(|| serde_json::from_str::<Value>(&capture.stdout).ok());
        let mut output = text_output(capture.stdout);
        output.value = value.flatten();
        output
    }
}

/// User-level only. Project `.yi/tools` waits on hash-pinned trust (D30): without a grant
/// store, a cloned repo's tools are arbitrary code behind one prompt.
#[derive(Debug)]
pub struct SkippedExecTool {
    pub path: PathBuf,
    pub reason: String,
}

pub fn discover_exec_tools(dir: &Path) -> (Vec<ExecTool>, Vec<SkippedExecTool>) {
    const PROBE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
    discover_within(dir, PROBE_DEADLINE)
}

fn discover_within(
    dir: &Path,
    deadline: std::time::Duration,
) -> (Vec<ExecTool>, Vec<SkippedExecTool>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return (Vec::new(), Vec::new());
    };
    let until = std::time::Instant::now() + deadline;
    let never: crate::tool::CancelFlag = std::sync::Arc::new(|| false);
    let (mut tools, mut skipped) = (Vec::new(), Vec::new());
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_executable_file(&path) {
            continue;
        }
        let remaining = until.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            skipped.push(SkippedExecTool {
                path,
                reason: format!("schema probe timed out after {}s", deadline.as_secs_f64()),
            });
            continue;
        }
        let mut command = crate::process::command(&path);
        command.arg("--schema");
        let capture = match run_captured_until(command, None, &never, remaining, OUTPUT_CAP) {
            Ok(capture) => capture,
            Err(reason) => {
                skipped.push(SkippedExecTool { path, reason });
                continue;
            }
        };
        let reason = if capture.cancelled {
            Some(format!(
                "schema probe timed out after {}s",
                deadline.as_secs_f64()
            ))
        } else if capture.exit_code != Some(0) {
            Some(match capture.exit_code {
                Some(code) => format!("--schema exited with code {code}"),
                None => "--schema terminated without an exit code".to_owned(),
            })
        } else {
            None
        };
        if let Some(reason) = reason {
            skipped.push(SkippedExecTool { path, reason });
            continue;
        }
        match ExecTool::from_schema_output(path.clone(), &capture.stdout) {
            Some(tool) => tools.push(tool),
            None => skipped.push(SkippedExecTool {
                path,
                reason: "no valid schema on stdout".to_owned(),
            }),
        }
    }
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    skipped.sort_by(|left, right| left.path.cmp(&right.path));
    (tools, skipped)
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

#[cfg(all(test, unix))]
mod tests {
    use super::discover_within;
    use crate::Tool;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn exec_tool_probes_share_one_deadline() -> TestResult {
        let dir = crate::scratch::Scratch::new("yi-tools-exec-shared-deadline")?;
        for name in ["hang-a", "hang-b"] {
            let script = dir.join(name);
            fs::write(&script, "#!/bin/sh\nsleep 30\n")?;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
        }
        let started = std::time::Instant::now();
        let (_tools, skipped) = discover_within(&dir, std::time::Duration::from_millis(300));
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
        assert_eq!(skipped.len(), 2);
        Ok(())
    }

    #[test]
    fn exec_tool_probes_stop_at_the_deadline() -> TestResult {
        let dir = crate::scratch::Scratch::new("yi-tools-exec-deadline")?;
        let script = dir.join("fast");
        fs::write(
            &script,
            "#!/bin/sh\nsleep 0.1\nprintf '{\"name\":\"fast\",\"input_schema\":{\"type\":\"object\"}}'\n",
        )?;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
        let (tools, skipped) = discover_within(&dir, std::time::Duration::from_millis(300));
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "fast");
        Ok(())
    }
}
