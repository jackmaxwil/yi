use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::tool::{
    DETAIL_CAP, Tool, ToolContext, ToolKind, ToolOutput, error_output, require_str, resolve_path,
    text_output,
};

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

    fn preview(&self, input: &Map<String, Value>, cwd: &Path) -> Option<String> {
        let path = input.get("path").and_then(Value::as_str)?;
        let content = input.get("content").and_then(Value::as_str)?;
        let resolved = if Path::new(path).is_absolute() {
            std::path::PathBuf::from(path)
        } else {
            cwd.join(path)
        };
        let before = fs::read_to_string(&resolved).unwrap_or_default();
        let patch = crate::diff::patch(&before, content, &resolved);
        (!patch.is_empty()).then(|| patch.as_str().to_owned())
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
        // Read before the write: nothing else reconstructs the ground it replaced. Past the
        // cap no base is read and no patch claimed, since a missing base reads as an add.
        let cap = u64::try_from(DETAIL_CAP).unwrap_or(u64::MAX);
        let before = match fs::metadata(&path) {
            Ok(meta) if meta.len() > cap => None,
            Ok(_) => Some(fs::read_to_string(&path).unwrap_or_default()),
            Err(_) => Some(String::new()),
        };
        match fs::write(&path, content) {
            Ok(()) => {
                if let Some(state) = &self.hashline {
                    crate::hashline::tool::record_write_snapshot(state, &path, content);
                }
                let mut output = text_output(format!(
                    "Wrote {} bytes to {}",
                    content.len(),
                    path.display()
                ));
                if let Some(before) = &before {
                    let patch = crate::diff::patch(before, content, &path);
                    if !patch.is_empty() {
                        output.result.details = crate::diff::patch_details(&patch);
                    }
                }
                output
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

/// Every other verb keeps the `Exec` default: the flag warns about blast
/// radius, and this is the set with none.
const READ_ONLY_VERBS: [&str; 24] = [
    "ls", "cat", "head", "tail", "wc", "pwd", "echo", "printf", "which", "type", "file", "stat",
    "du", "df", "date", "env", "printenv", "grep", "rg", "ag", "find", "fd", "diff", "true",
];

/// `git` is half a read tool: only the reporting subcommands qualify.
const READ_ONLY_GIT: [&str; 10] = [
    "log",
    "status",
    "diff",
    "show",
    "blame",
    "branch",
    "describe",
    "rev-parse",
    "ls-files",
    "shortlog",
];

/// Grid's query verbs read the chart; `survey` and `edit` write.
const READ_ONLY_GRID: [&str; 14] = [
    "resolve", "uses", "scope", "todo", "roots", "explain", "version", "orphans", "hotspots",
    "log", "diff", "drift", "check", "help",
];

/// Incident: every bash call reported as irreversible, so the advisor flagged `ls -la && git
/// log` into meaninglessness. Each segment is screened; all must be read-only.
fn read_only_command(command: &str) -> bool {
    command
        .split(['|', ';', '\n'])
        .flat_map(|part| part.split("&&"))
        .all(read_only_segment)
}

fn read_only_segment(segment: &str) -> bool {
    let tokens: Vec<&str> = segment
        .split_whitespace()
        // `2>/dev/null` overwrites nothing that exists.
        .filter(|token| !(token.contains('>') && token.ends_with("/dev/null")))
        .collect();
    if tokens
        .iter()
        .any(|token| token.contains('>') || token.contains('`') || token.contains("$("))
    {
        return false;
    }
    let mut words = tokens.iter().skip_while(|word| word.contains('='));
    let Some(verb) = words.next() else {
        return true;
    };
    let verb = verb.rsplit('/').next().unwrap_or(verb);
    match verb {
        "git" => words.next().is_some_and(|sub| READ_ONLY_GIT.contains(sub)),
        "grid" => words.next().is_some_and(|sub| READ_ONLY_GRID.contains(sub)),
        _ => READ_ONLY_VERBS.contains(&verb),
    }
}

/// Persisted per call so a stats pass can see shell searches the grep tool
/// should have served.
pub(crate) fn command_category(command: &str) -> &'static str {
    let mut categories = command
        .split(['|', ';', '\n'])
        .flat_map(|part| part.split("&&"))
        .filter_map(segment_category);
    let Some(first) = categories.next() else {
        return "unknown";
    };
    if categories.all(|category| category == first) {
        first
    } else {
        "mixed"
    }
}

fn segment_category(segment: &str) -> Option<&'static str> {
    let mut words = segment
        .split_whitespace()
        .skip_while(|word| word.contains('='));
    let verb = words.next()?;
    let verb = verb.rsplit('/').next().unwrap_or(verb);
    Some(match verb {
        "grep" | "rg" | "ag" => "search",
        "cat" | "head" | "tail" | "less" | "more" | "wc" | "file" | "stat" => "read",
        "ls" | "find" | "fd" | "tree" => "list_files",
        "git" | "jj" => "vcs",
        "cargo" | "just" => match words.next() {
            Some("test" | "nextest") => "test",
            _ => "build",
        },
        "make" | "npm" | "pnpm" | "yarn" | "go" | "rustc" | "cc" | "gcc" | "tsc" => "build",
        "pytest" => "test",
        "mkdir" | "touch" | "rm" | "mv" | "cp" | "chmod" | "ln" | "tee" => "write",
        // `sed -n 'Np'` is the read tool's clip escape hatch; only `-i` writes.
        "sed" => {
            if words.any(|word| word.starts_with("-i")) {
                "write"
            } else {
                "read"
            }
        }
        _ => "unknown",
    })
}

#[derive(Default)]
pub struct BashTool {
    pub hashline: Option<crate::hashline::tool::SharedHashline>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SkipReason {
    Compound,
    NotAView,
    NotOneFile,
}

enum Bridge {
    Tag { typed: String, path: PathBuf },
    Skip(SkipReason),
}

impl SkipReason {
    fn name(self) -> &'static str {
        match self {
            Self::Compound => "compound",
            Self::NotAView => "not-a-view",
            Self::NotOneFile => "not-one-file",
        }
    }
}

const VIEW_VERBS: [&str; 6] = ["cat", "head", "tail", "sed", "rg", "grep"];

/// One plain view of one existing file can anchor an edit: the command is a single segment,
/// its verb prints file text, and exactly one argument names a regular file.
fn bridge_target(command: &str, cwd: &Path) -> Bridge {
    if command.contains(['|', ';', '\n', '>', '`'])
        || command.contains("&&")
        || command.contains("$(")
    {
        return Bridge::Skip(SkipReason::Compound);
    }
    let mut tokens = command
        .split_whitespace()
        .skip_while(|token| token.contains('='));
    let verb = tokens
        .next()
        .map(|verb| verb.rsplit('/').next().unwrap_or(verb));
    if !verb.is_some_and(|verb| VIEW_VERBS.contains(&verb)) {
        return Bridge::Skip(SkipReason::NotAView);
    }
    let files: Vec<(String, PathBuf)> = tokens
        .filter(|token| !token.starts_with('-'))
        .map(|token| (token.to_owned(), cwd.join(token)))
        .filter(|(_, path)| path.is_file())
        .collect();
    match files.as_slice() {
        [(typed, path)] => Bridge::Tag {
            typed: typed.clone(),
            path: path.clone(),
        },
        _ => Bridge::Skip(SkipReason::NotOneFile),
    }
}

/// Bare `cat` shows every row; any other view shows the rows whose text is in the output.
fn viewed_lines(command: &str, content: &str, output: &str) -> Vec<u64> {
    let bare_cat = command
        .split_whitespace()
        .next()
        .is_some_and(|verb| verb.rsplit('/').next() == Some("cat"))
        && !command
            .split_whitespace()
            .any(|token| token.starts_with('-'));
    content
        .split('\n')
        .enumerate()
        .filter(|(_, line)| {
            let trimmed = line.trim();
            bare_cat || (trimmed.len() >= 3 && output.contains(trimmed))
        })
        .filter_map(|(index, _)| u64::try_from(index).ok()?.checked_add(1))
        .collect()
}

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
                "wait": {"type": "integer", "description": "Seconds to wait for that job, clamped to 5-300"},
                "max_output_lines": {"type": "integer", "description": "Per-call reducer line budget, for when the full output matters"}
            }
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Exec
    }

    fn kind_for(&self, input: &Map<String, Value>) -> ToolKind {
        match input.get("command").and_then(Value::as_str) {
            Some(command) if read_only_command(command) => ToolKind::Read,
            Some(_) | None => ToolKind::Exec,
        }
    }

    fn irreversible(&self, input: &Map<String, Value>) -> bool {
        match input.get("command").and_then(Value::as_str) {
            Some(command) => !read_only_command(command),
            None => false,
        }
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
            context.sandbox.as_ref(),
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
        let max_lines = input
            .get("max_output_lines")
            .and_then(Value::as_u64)
            .and_then(|lines| usize::try_from(lines).ok())
            .filter(|lines| *lines > 0);
        let reduced = crate::reduce::reduce(
            command,
            &capture.stdout,
            &capture.stderr,
            exit_code_for_reduce,
            context.recovery_dir.as_deref(),
            max_lines,
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
        if context.sandbox.is_some()
            && let Some(hint) = crate::sandbox::denial_hint(
                capture.exit_code,
                &format!("{}{}", capture.stdout, capture.stderr),
            )
        {
            sections.push(hint);
        }
        let mut text = if sections.is_empty() {
            "(no output)".to_owned()
        } else {
            sections.join("\n")
        };
        let bridge = match &self.hashline {
            Some(state) if exit_code == 0 && !capture.cancelled => {
                match bridge_target(command, &context.cwd) {
                    Bridge::Tag { typed, path } => match fs::read_to_string(&path) {
                        Ok(content) => {
                            let seen = viewed_lines(command, &content, &capture.stdout);
                            let tag = crate::hashline::tool::record_view_snapshot(
                                state, &path, &content, &seen,
                            );
                            let header =
                                crate::hashline::format::format_hashline_header(&typed, tag);
                            text = format!("{header}\n{text}");
                            "tag"
                        }
                        Err(_) => SkipReason::NotOneFile.name(),
                    },
                    Bridge::Skip(reason) => reason.name(),
                }
            }
            Some(_) | None => SkipReason::NotAView.name(),
        };
        let mut output = text_output(text);
        output.result.details = json!({
            "exitCode": exit_code,
            "truncated": capture.truncated,
            "cancelled": capture.cancelled,
            "rawBytes": reduced.raw_bytes,
            "outBytes": reduced.out_bytes,
            "category": command_category(command),
            "bridge": bridge,
        });
        output.is_error = exit_code != 0 || capture.cancelled;
        output
    }
}

/// The same tool with no command, never a second tool to discover.
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

/// Gitignore-filtered and sorted, for surfaces offering a file picker.
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
