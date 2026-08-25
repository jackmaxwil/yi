use std::fs;
use std::path::Path;

use serde_json::{Map, Value, json};

use crate::tool::{
    Tool, ToolContext, ToolKind, ToolOutput, error_output, require_str, resolve_path, text_output,
};

const MATCH_CAP: usize = 1_000;
const GREP_HIT_CAP: usize = 200;

#[derive(Default)]
pub struct WriteTool {
    pub hashline: Option<crate::hashline::tool::SharedHashline>,
}

impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Write content to a file, creating parent directories and overwriting any existing file."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File path (absolute, or relative to the working directory)"},
                "content": {"type": "string", "description": "Full file content to write"}
            },
            "required": ["path", "content"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Write
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let path = match require_str(&input, "path") {
            Ok(path) => resolve_path(context, path),
            Err(message) => return error_output(message),
        };
        let content = match require_str(&input, "content") {
            Ok(content) => content,
            Err(message) => return error_output(message),
        };
        if let Some(parent) = path.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            return error_output(format!("failed to create {}: {error}", parent.display()));
        }
        match fs::write(&path, content) {
            Ok(()) => {
                if let Some(state) = &self.hashline {
                    crate::hashline::tool::record_write_snapshot(state, &path, content);
                }
                text_output(format!(
                    "Wrote {} bytes to {}",
                    content.len(),
                    path.display()
                ))
            }
            Err(error) => error_output(format!("failed to write {}: {error}", path.display())),
        }
    }
}

pub(crate) fn walk_files(root: &Path, visit: &mut dyn FnMut(&Path) -> bool) {
    let mut ignore = crate::ignore::Ignore::default();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        ignore.push_dir(&dir);
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                if !ignore.ignored(&path, true) {
                    stack.push(path);
                }
            } else if file_type.is_file() && !ignore.ignored(&path, false) && !visit(&path) {
                return;
            }
        }
    }
}

pub struct GlobTool;

impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files whose path matches a glob pattern (e.g. \"**/*.rs\"), searching under path or the working directory."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Glob pattern, matched against the path relative to the search root"},
                "path": {"type": "string", "description": "Directory to search (default: the working directory)"}
            },
            "required": ["pattern"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let pattern = match require_str(&input, "pattern") {
            Ok(pattern) => pattern,
            Err(message) => return error_output(message),
        };
        let matcher = match globset::GlobBuilder::new(pattern)
            .literal_separator(false)
            .build()
        {
            Ok(glob) => glob.compile_matcher(),
            Err(error) => return error_output(format!("invalid glob pattern: {error}")),
        };
        let root = input
            .get("path")
            .and_then(Value::as_str)
            .map_or_else(|| context.cwd.clone(), |path| resolve_path(context, path));
        let mut matches: Vec<String> = Vec::new();
        walk_files(&root, &mut |path| {
            let relative = path.strip_prefix(&root).unwrap_or(path);
            if matcher.is_match(relative) {
                matches.push(path.display().to_string());
            }
            matches.len() < MATCH_CAP
        });
        matches.sort();
        if matches.is_empty() {
            text_output("No files matched")
        } else if matches.len() >= MATCH_CAP {
            text_output(format!(
                "{}\n[result capped at {MATCH_CAP} matches]",
                matches.join("\n")
            ))
        } else {
            text_output(matches.join("\n"))
        }
    }
}

pub struct GrepTool;

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents for a literal substring under path or the working directory. Returns path:line:text hits."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Literal substring to search for (not a regex)"},
                "path": {"type": "string", "description": "Directory or file to search (default: the working directory)"},
                "ignore_case": {"type": "boolean", "description": "Case-insensitive search"}
            },
            "required": ["pattern"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let pattern = match require_str(&input, "pattern") {
            Ok(pattern) => pattern,
            Err(message) => return error_output(message),
        };
        let ignore_case = input
            .get("ignore_case")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let needle = if ignore_case {
            pattern.to_lowercase()
        } else {
            pattern.to_owned()
        };
        let root = input
            .get("path")
            .and_then(Value::as_str)
            .map_or_else(|| context.cwd.clone(), |path| resolve_path(context, path));
        let mut hits: Vec<String> = Vec::new();
        let mut search_file = |path: &Path| -> bool {
            let Ok(bytes) = fs::read(path) else {
                return true;
            };
            if bytes.iter().take(4096).any(|byte| *byte == 0) {
                return true;
            }
            let content = String::from_utf8_lossy(&bytes);
            for (index, line) in content.split('\n').enumerate() {
                let haystack = if ignore_case {
                    line.to_lowercase()
                } else {
                    line.to_owned()
                };
                if haystack.contains(&needle) {
                    hits.push(format!("{}:{}:{}", path.display(), index + 1, line));
                    if hits.len() >= GREP_HIT_CAP {
                        return false;
                    }
                }
            }
            true
        };
        if root.is_file() {
            search_file(&root);
        } else {
            walk_files(&root, &mut search_file);
        }
        if hits.is_empty() {
            text_output("No matches found")
        } else if hits.len() >= GREP_HIT_CAP {
            text_output(format!(
                "{}\n[result capped at {GREP_HIT_CAP} hits]",
                hits.join("\n")
            ))
        } else {
            text_output(hits.join("\n"))
        }
    }
}

pub struct BashTool;

impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Run a shell command with sh -c in the working directory and return its output and exit code."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command to run. Omit it to check on a background job instead."},
                "job": {"type": "integer", "description": "Background job to check on; defaults to the most recent"},
                "wait": {"type": "integer", "description": "Seconds to wait for that job, clamped to 5-300"}
            }
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Exec
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let command = input
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if command.trim().is_empty() {
            return poll_job(&input);
        }
        let capture = match crate::jobs::run_or_background(
            command,
            &context.cwd,
            &context.cancelled,
            context.auto_background,
        ) {
            Ok(crate::jobs::Run::Finished(capture)) => *capture,
            Ok(crate::jobs::Run::Backgrounded(id)) => {
                let mut output = text_output(format!(
                    "Backgrounded as job {id}. Call bash with no command (optionally job={id}) to check on it."
                ));
                output.result.details = json!({ "job": id.0, "backgrounded": true });
                return output;
            }
            Err(message) => return error_output(message),
        };
        let exit_code_for_reduce = capture.exit_code.unwrap_or(-1);
        let reduced = crate::reduce::reduce(
            command,
            &capture.stdout,
            &capture.stderr,
            exit_code_for_reduce,
            context.recovery_dir.as_deref(),
        );
        let mut sections = Vec::new();
        if !reduced.text.is_empty() {
            sections.push(reduced.text.clone());
        }
        if capture.truncated {
            sections.push("[output truncated]".to_owned());
        }
        if capture.cancelled {
            sections.push("[command aborted]".to_owned());
        }
        let exit_code = capture.exit_code.unwrap_or(-1);
        if exit_code != 0 {
            sections.push(format!("exit code: {exit_code}"));
        }
        let text = if sections.is_empty() {
            "(no output)".to_owned()
        } else {
            sections.join("\n")
        };
        let mut output = text_output(text);
        output.result.details = json!({
            "exitCode": exit_code,
            "truncated": capture.truncated,
            "cancelled": capture.cancelled,
            "rawBytes": reduced.raw_bytes,
            "outBytes": reduced.out_bytes,
        });
        output.is_error = exit_code != 0 || capture.cancelled;
        output
    }
}

/// T12: checking on a job is the same tool with no command, never a second
/// `jobs` tool the model has to discover.
fn poll_job(input: &Map<String, Value>) -> ToolOutput {
    let requested = input
        .get("job")
        .and_then(Value::as_u64)
        .map(crate::jobs::JobId);
    let deadline = input
        .get("wait")
        .and_then(Value::as_u64)
        .map(crate::jobs::clamp_wait);
    let started = std::time::Instant::now();
    loop {
        let report = match requested {
            Some(id) => crate::jobs::registry().report(id),
            None => crate::jobs::registry().latest(),
        };
        let Some(report) = report else {
            return error_output("no background job to check on".to_owned());
        };
        if report.finished {
            let exit_code = report.exit_code.unwrap_or(-1);
            let mut output = text_output(format!(
                "job {} finished (exit {exit_code}): {}\n{}",
                report.id, report.command, report.output
            ));
            output.result.details = json!({ "job": report.id.0, "exitCode": exit_code });
            output.is_error = exit_code != 0;
            return output;
        }
        match deadline {
            Some(limit) if started.elapsed() < limit => {
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            _ => {
                let mut output = text_output(format!(
                    "job {} still running: {}",
                    report.id, report.command
                ));
                output.result.details = json!({ "job": report.id.0, "running": true });
                return output;
            }
        }
    }
}

/// Relative paths under `root`, gitignore-filtered and sorted, for surfaces
/// that offer a file picker (TUI `@`).
pub fn list_files(root: &Path, cap: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    walk_files(root, &mut |path| {
        if let Ok(relative) = path.strip_prefix(root) {
            out.push(relative.to_string_lossy().into_owned());
        }
        out.len() < cap
    });
    out.sort();
    out
}
