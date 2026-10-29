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

/// Invariant: probes run side by side, each with its own deadline, so a slow sibling enumerated
/// first can neither skip a later tool nor stretch discovery past one deadline.
fn discover_within(
    dir: &Path,
    deadline: std::time::Duration,
) -> (Vec<ExecTool>, Vec<SkippedExecTool>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return (Vec::new(), Vec::new());
    };
    let paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| is_executable_file(path))
        .collect();
    let probed: Vec<Result<ExecTool, SkippedExecTool>> = std::thread::scope(|scope| {
        let probes: Vec<_> = paths
            .into_iter()
            .map(|path| scope.spawn(move || probe(path, deadline)))
            .collect();
        probes
            .into_iter()
            .filter_map(|probe| probe.join().ok())
            .collect()
    });
    let (mut tools, mut skipped) = (Vec::new(), Vec::new());
    for outcome in probed {
        match outcome {
            Ok(tool) => tools.push(tool),
            Err(skip) => skipped.push(skip),
        }
    }
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    skipped.sort_by(|left, right| left.path.cmp(&right.path));
    (tools, skipped)
}

fn probe(path: PathBuf, deadline: std::time::Duration) -> Result<ExecTool, SkippedExecTool> {
    let mut command = crate::process::command(&path);
    command.arg("--schema");
    let capture = match run_captured_until(command, None, None, deadline, OUTPUT_CAP) {
        Ok(capture) => capture,
        Err(reason) => return Err(SkippedExecTool { path, reason }),
    };
    let reason = if capture.cancelled {
        format!("schema probe timed out after {}s", deadline.as_secs_f64())
    } else if capture.exit_code != Some(0) {
        match capture.exit_code {
            Some(code) => format!("--schema exited with code {code}"),
            None => "--schema terminated without an exit code".to_owned(),
        }
    } else {
        return ExecTool::from_schema_output(path.clone(), &capture.stdout).ok_or(
            SkippedExecTool {
                path,
                reason: "no valid schema on stdout".to_owned(),
            },
        );
    };
    Err(SkippedExecTool { path, reason })
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

    #[test]
    fn a_slow_sibling_cannot_skip_a_later_tool() -> TestResult {
        let dir = crate::scratch::Scratch::new("yi-tools-exec-slow-siblings")?;
        for name in ["slow-a", "slow-b"] {
            let script = dir.join(name);
            fs::write(
                &script,
                format!("#!/bin/sh\nsleep 0.2\nprintf '{{\"name\":\"{name}\"}}'\n"),
            )?;
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;
        }
        let (tools, skipped) = discover_within(&dir, std::time::Duration::from_millis(300));
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(tools.len(), 2);
        Ok(())
    }
}
